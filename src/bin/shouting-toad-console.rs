//! Windows console build of the same program, shipped as `shouting-toad.com` next to the GUI
//! `shouting-toad.exe`. Terminals prefer `.com` over `.exe` (PATHEXT), so typing `shouting-toad`
//! in cmd or PowerShell runs this one: the terminal waits for it, shows its output and gets its
//! exit code. Shortcuts and double-clicks still start the GUI build.

fn main() {
    shouting_toad::entry::run();
}
