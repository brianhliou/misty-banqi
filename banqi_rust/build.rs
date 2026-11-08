fn main() {
    // Supply the platform linker flags when building the Python extension
    // directly with Cargo as well as through maturin.
    pyo3_build_config::add_extension_module_link_args();
}
