//! Context menu placeholder.
//!
//! This module provides placeholder implementations for the context menu
/// functionality. The real implementation will use TrackPopupMenu API.

use windows::Win32::Foundation::{HWND};
use windows::Win32::UI::WindowsAndMessaging::{HMENU, TPM_LEFTALIGN, TPM_RETURNCMD};

/// Menu item identifiers
pub const IDM_TOPMOST: u16 = 1;
pub const IDM_MINIMAL: u16 = 2;
pub const IDM_HOVER: u16 = 3;
pub const IDM_BGCLR: u16 = 6;
pub const IDM_FGCLR: u16 = 7;
pub const IDM_CLRDEF: u16 = 8;
pub const IDM_QUIT: u16 = 9;

/// Placeholder context menu state
pub struct MenuState {
    /// Whether topmost mode is enabled
    pub topmost: bool,
    /// Whether minimal mode is enabled
    pub minimal: bool,
    /// Whether hover mode is enabled
    pub hover: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            topmost: true,
            minimal: false,
            hover: false,
        }
    }
}

/// Show the context menu at the current cursor position.
///
/// This is a placeholder that will be implemented with TrackPopupMenu
/// in a later phase.
pub fn show_menu(_hwnd: HWND) -> Option<u16> {
    // Placeholder: menu handling not yet implemented
    // Real implementation will:
    // 1. Create popup menu with TrackPopupMenu
    // 2. Return selected command ID
    None
}

/// Handle a menu command.
///
/// This processes the selected menu item.
pub fn handle_command(_cmd: u16) {
    // Placeholder: menu command handling not yet implemented
    match _cmd {
        IDM_QUIT => {
            // Will post WM_CLOSE
        }
        IDM_TOPMOST => {
            // Will toggle topmost mode
        }
        IDM_MINIMAL => {
            // Will toggle minimal mode
        }
        IDM_HOVER => {
            // Will toggle hover mode
        }
        _ => {}
    }
}
