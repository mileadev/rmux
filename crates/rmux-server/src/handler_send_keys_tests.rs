use super::super::input_capture::RawPaneInputProbe;

#[path = "handler_send_keys_tests/live_attach.rs"]
mod live_attach;

#[path = "handler_send_keys_tests/read_only_detach.rs"]
mod read_only_detach;

#[path = "handler_send_keys_tests/read_only_navigation_security.rs"]
mod read_only_navigation_security;

#[path = "handler_send_keys_tests/kitty_keyboard.rs"]
mod kitty_keyboard;

#[path = "handler_send_keys_tests/bracketed_paste_live.rs"]
mod bracketed_paste_live;

#[path = "handler_send_keys_tests/bracketed_paste_large.rs"]
mod bracketed_paste_large;

#[path = "handler_send_keys_tests/bracketed_paste_final_sink.rs"]
mod bracketed_paste_final_sink;

#[path = "handler_send_keys_tests/kitty_graphics_live.rs"]
mod kitty_graphics_live;

#[path = "handler_send_keys_tests/palette_modal.rs"]
mod palette_modal;

#[path = "handler_send_keys_tests/synchronize_panes.rs"]
mod synchronize_panes;
