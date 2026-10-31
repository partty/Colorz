//! Compiles `data/` resources (the app icon) into the binary.

fn main() {
    glib_build_tools::compile_resources(
        &["../data"],
        "../data/io.github.partty.Colorz.gresource.xml",
        "colorz.gresource",
    );
}
