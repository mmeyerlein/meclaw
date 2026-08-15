use clap::Parser;

#[test]
fn rescan_templates_flag_default_is_false() {
    let args = meclaw_cli::Args::try_parse_from(["meclaw"]).unwrap();
    assert!(!args.rescan_templates);
}

#[test]
fn rescan_templates_flag_can_be_set() {
    let args = meclaw_cli::Args::try_parse_from(["meclaw", "--rescan-templates"]).unwrap();
    assert!(args.rescan_templates);
}

#[test]
fn templates_path_flag_default_uses_root() {
    let args = meclaw_cli::Args::try_parse_from(["meclaw"]).unwrap();
    assert!(args.templates.is_none()); // None = "default: <root>/templates"
}

#[test]
fn templates_path_flag_can_override() {
    let args = meclaw_cli::Args::try_parse_from(["meclaw", "--templates", "/custom/path"]).unwrap();
    assert_eq!(
        args.templates.as_deref(),
        Some(std::path::Path::new("/custom/path"))
    );
}
