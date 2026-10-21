# events

Local working folder for event editions. Everything in here except this README is ignored by git,
so event configurations and signing keys stay on this machine.

Use one subfolder per event:

```powershell
cargo build --release --bin lancast-event
mkdir events\netgame-2026
target\release\lancast-event keygen events\netgame-2026\secret.key     # prints the public key
copy packaging\event.example.toml events\netgame-2026\event.toml
target\release\lancast-event sign events\netgame-2026\event.toml events\netgame-2026\secret.key events\netgame-2026\LANcast.event
powershell -File tools\package.ps1 -EventConfig events\netgame-2026\event.toml -SigningKey events\netgame-2026\secret.key
```

Back up `secret.key` somewhere safe: without it, the event's configuration can't be signed again.
Details: [`docs/EVENT_LINKS.md`](../docs/EVENT_LINKS.md#event-edition-lan-distro).
