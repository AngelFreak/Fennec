fn main() {
    // The mockup's icons, bundled so they look the same under any icon theme.
    glib_build_tools::compile_resources(
        &["data/icons/ui/scalable/actions"],
        "data/icons/ui/icons.gresource.xml",
        "icons.gresource",
    );
}
