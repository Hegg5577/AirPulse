fn main() -> anyhow::Result<()> {
    for candidate in keyphore_lib::hid::enumerate_air75_candidates()? {
        println!(
            "candidate: product={:?}, bus={:?}, interface={}, usage={:04x}:{:04x}, path={}",
            candidate.product,
            candidate.bus,
            candidate.interface_number,
            candidate.usage_page,
            candidate.usage,
            candidate.path
        );
    }
    let report = keyphore_lib::hid::probe_read_only()?;
    println!(
        "Air75 V3: interface={}, usage={:04x}:{:04x}",
        report.descriptor.interface_number, report.descriptor.usage_page, report.descriptor.usage
    );
    println!(
        "state={}",
        report
            .state
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}
