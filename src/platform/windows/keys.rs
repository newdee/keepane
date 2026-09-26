//! A key, as a pane's program is to get it: on Windows the console's own
//! record, in win32-input-mode (what ConPTY reads back into the same record,
//! so PSReadLine, IME and every modifier arrive as typed).

use crate::ipc::KeyRecord;

/// The bytes that give `rec` to a pane. (`_app_cursor` is for terminals that
/// take keys as VT sequences; the record carries everything here.)
pub fn to_pane(rec: &KeyRecord, _app_cursor: bool) -> Vec<u8> {
    crate::server::input::encode_key_record(rec)
}
