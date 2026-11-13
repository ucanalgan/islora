use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_SPACE,
};

const SUMMON: i32 = 1;

/// Tried in order, since another app may already own a global shortcut.
/// Ctrl+Alt with a letter is avoided: it collides with AltGr, which types
/// characters such as `@` on Turkish and many other layouts.
const SHORTCUTS: [HOT_KEY_MODIFIERS; 2] = [
    HOT_KEY_MODIFIERS(MOD_WIN.0 | MOD_ALT.0),
    HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_ALT.0),
];

/// Returns false if every shortcut is taken. The island still works without
/// one, through the top edge, so this is not an error.
pub fn register(hwnd: HWND) -> bool {
    SHORTCUTS.iter().any(|&modifiers| unsafe {
        RegisterHotKey(
            Some(hwnd),
            SUMMON,
            modifiers | MOD_NOREPEAT,
            VK_SPACE.0 as u32,
        )
        .is_ok()
    })
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), SUMMON);
    }
}
