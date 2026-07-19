//! Input driver subsystem — USB HID and virtio-input.

pub mod usb_hid;
#[cfg(target_arch = "x86_64")]
pub mod virtio_input;
