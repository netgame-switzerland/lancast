//! Stub of `dialog` for platforms other than Windows (D32): no native
//! dialog; the default button is chosen.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Information,
    Warning,
    Shield,
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub footer: Option<String>,
    pub icon: Icon,
    pub buttons: Vec<String>,
    pub default: usize,
    pub cancel: usize,
}

pub fn show(_owner: Option<isize>, dialog: Dialog, done: impl FnOnce(usize) + Send + 'static) {
    done(dialog.default);
}
