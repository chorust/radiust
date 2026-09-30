#[cfg(feature = "extension-module")]
use pyo3::prelude::*;

#[cfg(feature = "extension-module")]
#[pyfunction]
fn cli_main(args: Vec<String>) -> u8 {
    radiust_cli::run_args(args)
}

#[cfg(feature = "extension-module")]
#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    radiust_core::python::register(module)?;
    radiust_core::python_types::register(module)?;
    module.add_function(wrap_pyfunction!(cli_main, module)?)?;
    Ok(())
}
