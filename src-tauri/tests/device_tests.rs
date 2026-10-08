use keyphore_lib::device::{Bus, DeviceDescriptor, DeviceSelectionError, select_control_interface};

fn air75() -> DeviceDescriptor {
    DeviceDescriptor {
        path: "control".into(),
        vendor_id: 0x19f5,
        product_id: 0x1028,
        product: "Air75 V3".into(),
        bus: Bus::Usb,
        interface_number: 3,
        usage_page: 1,
        usage: 0,
    }
}

#[test]
fn selects_the_verified_air75_usb_control_interface() {
    assert_eq!(
        select_control_interface(&[air75()]).unwrap().path,
        "control"
    );
}

#[test]
fn rejects_wireless_unknown_and_changed_interfaces() {
    let mut wireless = air75();
    wireless.bus = Bus::Bluetooth;
    let mut unknown = air75();
    unknown.product = "Air75 V3 ISO".into();
    let mut changed = air75();
    changed.interface_number = 4;
    for device in [wireless, unknown, changed] {
        assert_eq!(
            select_control_interface(&[device]).unwrap_err(),
            DeviceSelectionError::NoVerifiedControlInterface
        );
    }
}

#[test]
fn rejects_ambiguous_control_interfaces() {
    assert_eq!(
        select_control_interface(&[air75(), air75()]).unwrap_err(),
        DeviceSelectionError::Ambiguous
    );
}
