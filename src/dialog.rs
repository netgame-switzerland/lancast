//! Native Windows dialogs (task dialogs, comctl32 v6) for confirmations.
//! Each dialog runs on its own thread, owned by the main window: modal for
//! the user, while egui keeps drawing (D42).

use tracing::warn;
use windows::{
    Win32::{
        Foundation::HWND,
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
        UI::{
            Controls::{
                TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TD_INFORMATION_ICON, TD_SHIELD_ICON,
                TD_WARNING_ICON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW,
            },
            WindowsAndMessaging::{
                IDOK, MB_ICONINFORMATION, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW,
            },
        },
    },
    core::{BOOL, HRESULT, HSTRING, PCWSTR, s, w},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Information,
    Warning,
    /// A security shield: settings vouched for by a signature.
    Shield,
}

/// A question with custom buttons.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// Window title.
    pub title: String,
    /// The question, in the dialog's large font.
    pub instruction: String,
    pub content: String,
    /// Small print at the bottom, with an information icon.
    pub footer: Option<String>,
    pub icon: Icon,
    /// Button labels, left to right.
    pub buttons: Vec<String>,
    /// Button chosen by Enter.
    pub default: usize,
    /// Button reported for Esc or the close box.
    pub cancel: usize,
}

/// Shows `dialog` owned by the window `owner` and calls `done` with the
/// index of the chosen button. Returns at once.
pub fn show(owner: Option<isize>, dialog: Dialog, done: impl FnOnce(usize) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("dialog".into())
        .spawn(move || {
            let owner = HWND(owner.unwrap_or(0) as *mut _);
            let choice = task_dialog(owner, &dialog).unwrap_or_else(|| message_box(owner, &dialog));
            done(choice);
        });
    if let Err(err) = spawned {
        warn!("cannot show a dialog: {err}");
    }
}

type TaskDialogIndirectFn =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

/// Button ids: index + this, clear of the predefined IDOK etc.
const BUTTON_ID_BASE: i32 = 100;

/// `None` if task dialogs are unavailable (no comctl32 v6 manifest).
fn task_dialog(owner: HWND, dialog: &Dialog) -> Option<usize> {
    // Resolved at runtime: without the manifest (examples, tests) comctl32
    // v5 is loaded, which lacks the function; a static import would keep
    // those executables from starting.
    // SAFETY: plain loader calls; the module stays loaded for the process.
    let function = unsafe {
        let module = LoadLibraryW(w!("comctl32.dll")).ok()?;
        GetProcAddress(module, s!("TaskDialogIndirect"))?
    };
    // SAFETY: TaskDialogIndirect has exactly this signature (CommCtrl.h).
    let task_dialog_indirect: TaskDialogIndirectFn = unsafe { std::mem::transmute(function) };

    let title = HSTRING::from(&dialog.title);
    let instruction = HSTRING::from(&dialog.instruction);
    let content = HSTRING::from(&dialog.content);
    let footer = dialog.footer.as_deref().map(HSTRING::from);
    let labels: Vec<HSTRING> = dialog.buttons.iter().map(HSTRING::from).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| TASKDIALOG_BUTTON {
            nButtonID: BUTTON_ID_BASE + i as i32,
            pszButtonText: PCWSTR(label.as_ptr()),
        })
        .collect();
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: BUTTON_ID_BASE + dialog.default as i32,
        ..Default::default()
    };
    config.Anonymous1.pszMainIcon = match dialog.icon {
        Icon::Information => TD_INFORMATION_ICON,
        Icon::Warning => TD_WARNING_ICON,
        Icon::Shield => TD_SHIELD_ICON,
    };
    if let Some(footer) = &footer {
        config.pszFooter = PCWSTR(footer.as_ptr());
        config.Anonymous2.pszFooterIcon = TD_INFORMATION_ICON;
    }
    let mut pressed = 0i32;
    // SAFETY: `config` and every string and button it points to live until
    // the call returns; the other out-pointers may be null.
    let result = unsafe {
        task_dialog_indirect(
            &raw const config,
            &raw mut pressed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result.is_err() {
        warn!("task dialog failed: {result:?}");
        return None;
    }
    let index = pressed - BUTTON_ID_BASE;
    Some(if (0..dialog.buttons.len() as i32).contains(&index) {
        index as usize
    } else {
        dialog.cancel
    })
}

/// Fallback: OK stands for the default button, Cancel for the cancel one.
fn message_box(owner: HWND, dialog: &Dialog) -> usize {
    let text = HSTRING::from(format!("{}\n\n{}", dialog.instruction, dialog.content));
    let title = HSTRING::from(&dialog.title);
    let icon = match dialog.icon {
        Icon::Warning => MB_ICONWARNING,
        Icon::Information | Icon::Shield => MB_ICONINFORMATION,
    };
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe { MessageBoxW(Some(owner), &text, &title, MB_OKCANCEL | icon) };
    if result == IDOK {
        dialog.default
    } else {
        dialog.cancel
    }
}
