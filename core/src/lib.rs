#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]

use log::info;
use pyo3::exceptions::{KeyError, RuntimeError, TypeError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use pyo3::wrap_pyfunction;
use std::convert::TryFrom;


#[pymodule]
fn radCAD(_py: Python, m: &PyModule) -> PyResult<()> {
    pyo3_log::init();

    info!("Initializing radCAD");

    m.add_class::<Model>()?;
    m.add_class::<Simulation>()?;
    m.add_wrapped(wrap_pyfunction!(run))?;
    m.add_wrapped(wrap_pyfunction!(single_run))?;
    m.add_wrapped(wrap_pyfunction!(generate_parameter_sweep))?;

    Ok(())
}

#[pyclass(subclass)]
#[derive(Debug, Clone)]
struct Model {
    #[pyo3(get, set)]
    initial_state: PyObject,
    #[pyo3(get, set)]
    state_update_blocks: PyObject,
    #[pyo3(get, set)]
    params: PyObject,
}

#[pymethods]
impl Model {
    #[new]
    fn new(initial_state: PyObject, state_update_blocks: PyObject, params: PyObject) -> Self {
        info!("New Model created");
        Model {
            initial_state,
            state_update_blocks,
            params,
        }
    }
}

#[pyclass(subclass)]
#[derive(Debug, Clone)]
struct Simulation {
    #[pyo3(get, set)]
    model: Model,
    #[pyo3(get, set)]
    timesteps: usize,
    #[pyo3(get, set)]
    runs: usize,
}

#[pymethods]
impl Simulation {
    #[new]
    #[args(timesteps = "100", runs = "1")]
    fn new(timesteps: usize, runs: usize, model: Model) -> Self {
        info!("New Simulation created");
        Simulation {
            timesteps,
            runs,
            model,
        }
    }
}

#[pyfunction]
fn run(simulations: &PyList) -> PyResult<PyObject> {
    let gil = Python::acquire_gil();
    let py = gil.python();
    let result: &PyList = PyList::empty(py);

    for (simulation_index, simulation_) in simulations.iter().enumerate() {
        let simulation: &Simulation = &simulation_.extract::<Simulation>()?;
        let timesteps = simulation.timesteps;
        let runs = simulation.runs;
        let initial_state: &PyDict = simulation.model.initial_state.extract(py)?;
        let state_update_blocks: &PyList = simulation.model.state_update_blocks.extract(py)?;
        let params: &PyDict = simulation.model.params.extract(py)?;

        let param_sweep_result = generate_parameter_sweep(py, params).unwrap();
        let param_sweep: &PyList = param_sweep_result.cast_as::<PyList>(py)?;

        for run in 0..runs {
            if !param_sweep.is_empty() {
                for (subset, param_set) in param_sweep.iter().enumerate() {
                    result
                        .call_method(
                            "extend",
                            (single_run(
                                py,
                                simulation_index,
                                timesteps,
                                run,
                                subset,
                                initial_state,
                                state_update_blocks,
                                param_set.extract()?,
                            )?,),
                            None,
                        )
                        .unwrap();
                }
            } else {
                result
                    .call_method(
                        "extend",
                        (single_run(
                            py,
                            simulation_index,
                            timesteps,
                            run,
                            0,
                            initial_state,
                            state_update_blocks,
                            params,
                        )?,),
                        None,
                    )
                    .unwrap();
            }
        }
    }
    Ok(result.into())
}

#[pyfunction]
fn single_run(
    py: Python,
    simulation: usize,
    timesteps: usize,
    run: usize,
    subset: usize,
    initial_state: &PyDict,
    state_update_blocks: &PyList,
    params: &PyDict,
) -> PyResult<PyObject> {
    info!("Starting run {}", run);
    // let copy = PyModule::import(py, "copy").expect("Failed to import Python copy module");
    let pickle = PyModule::import(py, "pickle").expect("Failed to import Python pickle module");
    let result: &PyList = PyList::empty(py);
    initial_state.set_item("simulation", simulation).unwrap();
    initial_state.set_item("subset", subset).unwrap();
    initial_state.set_item("run", run + 1).unwrap();
    initial_state.set_item("substep", 0).unwrap();
    initial_state.set_item("timestep", 0).unwrap();
    let initial_state_list = PyList::empty(py);
    initial_state_list.append(initial_state.copy()?).unwrap();
    result.append(initial_state_list).unwrap();
    unsafe {
        for timestep in 0..timesteps {
            let _pool = pyo3::GILPool::new(); // Frees GIL memory. Requires unsafe code block.
            let previous_state: &PyDict = match timestep {
                0 => {
                    result
                        .get_item(0)
                        .cast_as::<PyList>()?
                        .get_item(0)
                        .cast_as::<PyDict>()?
                        .copy()?
                },
                _ => {
                    let substates = result
                        .get_item(isize::try_from(result.len() - 1).expect("Failed to fetch previous state"))
                        .cast_as::<PyList>()?;
                    substates
                        .get_item(isize::try_from(substates.len() - 1).expect("Failed to fetch previous state"))
                        .cast_as::<PyDict>()?
                        .copy()?
                }
            };
            previous_state.set_item("simulation", simulation).unwrap();
            previous_state.set_item("subset", subset).unwrap();
            previous_state.set_item("run", run + 1).unwrap();
            previous_state.set_item("timestep", timestep + 1).unwrap();
            let substeps: &PyList = PyList::empty(py);
            for (substep, psu) in state_update_blocks.into_iter().enumerate() {
                let substate: &PyDict = match substep {
                    0 => previous_state.copy()?,
                    _ => substeps
                        .get_item(isize::try_from(substep - 1).expect("Failed to convert substep type"))
                        .cast_as::<PyDict>()?
                        .copy()?,
                };
                substate
                    .set_item("substep", substep + 1)
                    .expect("Failed to set substep state");
                for (state, function) in psu
                    .get_item("variables")
                    .expect("Get variables failed")
                    .cast_as::<PyDict>()
                    .expect("Get variables failed")
                    .iter()
                {
                    if !initial_state.contains(state)? {
                        return Err(PyErr::new::<KeyError, _>(
                            "Invalid state key in partial state update block",
                        ));
                    };
                    // let substate_copy: &PyDict = copy.call1("deepcopy", (substate,)).expect("Failed to deepcopy substate").extract().expect("Failed to extract substate deepcopy");
                    let substate_dump = pickle.call1("dumps", (substate, -1,)).expect("Failed to pickle.dump substate");
                    let substate_copy: &PyDict = pickle.call1("loads", (substate_dump,)).expect("Failed to pickle.loads substate").extract().expect("Failed to extract substate deep copy");
                    let signals = match reduce_signals(
                        py,
                        params,
                        substep,
                        result,
                        substate_copy,
                        psu.cast_as::<PyDict>()
                            .expect("Failed to cast partial state update block as dictionary"),
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(PyErr::new::<RuntimeError, _>(e));
                        }
                    };
                    let state_update: &PyTuple = match function.is_callable() {
                        true => {
                            match function.call(
                                (
                                    params,
                                    substep,
                                    result,
                                    substate_copy,
                                    signals
                                        .extract::<&PyDict>(py)
                                        .expect("Failed to convert policy signals to dictionary")
                                        .clone(),
                                ),
                                None,
                            ) {
                                Ok(v) => match v.extract() {
                                    Ok(v) => v,
                                    Err(_e) => return Err(PyErr::new::<RuntimeError, _>(
                                        "Failed to extract state update function result as tuple",
                                    )),
                                },
                                Err(e) => return Err(PyErr::new::<RuntimeError, _>(e)),
                            }
                        }
                        false => {
                            return Err(PyErr::new::<TypeError, _>(
                                "State update function is not callable",
                            ));
                        }
                    };
                    let state_key = state_update.get_item(0);
                    let state_value = state_update.get_item(1);
                    if !initial_state.contains(state_key)? {
                        return Err(PyErr::new::<KeyError, _>(
                            "Invalid state key returned from state update function",
                        ));
                    };
                    match state.downcast::<PyString>()?.to_string()?
                        == state_key.downcast::<PyString>()?.to_string()?
                    {
                        true => substate
                            .set_item(state_key, state_value)
                            .expect("Failed to update state"),
                        _ => {
                            return Err(PyErr::new::<KeyError, _>(format!(
                                "PSU state key {} doesn't match function state key {}",
                                state, state_key
                            )))
                        }
                    }
                }
                substeps
                    .insert(
                        isize::try_from(substep).expect("Failed to convert substep type"),
                        substate,
                    )
                    .expect("Failed to insert substep");
            }
            result.append(substeps).unwrap();
        }
    }
    Ok(result.into())
}

#[pyfunction]
fn generate_parameter_sweep(py: Python, params: &PyDict) -> PyResult<PyObject> {
    let param_sweep = PyList::empty(py);
    let mut max_len = 0;

    for value in params.values() {
        if value.len()? > max_len {
            max_len = value.len()?;
        }
    }

    for sweep_index in 0..max_len {
        let param_set = PyDict::new(py);
        for (key, value) in params.iter() {
            let param = if sweep_index < value.len()? {
                value.get_item(sweep_index)?
            } else {
                value.get_item(value.len()? - 1)?
            };
            param_set.set_item(key, param)?;
        }
        param_sweep.append(param_set)?;
    }

    Ok(param_sweep.into())
}

fn reduce_signals(
    py: Python,
    params: &PyDict,
    substep: usize,
    result: &PyList,
    substate: &PyDict,
    psu: &PyDict,
) -> PyResult<PyObject> {
    let mut policy_results = Vec::<&PyDict>::with_capacity(psu.len());
    for (_var, function) in psu
        .get_item("policies")
        .expect("Get policies failed")
        .cast_as::<PyDict>()
        .expect("Get policies failed")
        .iter()
    {
        match function.call((params, substep, result, substate), None) {
            Ok(v) => {
                policy_results.push(match v.extract::<&PyDict>() {
                    Ok(v) => v,
                    Err(_e) => return Err(PyErr::new::<RuntimeError, _>(
                        "Failed to extract policy function result as dictionary",
                    )),
                });
            }
            Err(e) => {
                // e.restore(py);
                // let s: String = py.eval(r#"
                // import traceback
                // traceback.format_exception()
                // "#, None, None)?.extract()?;
                // let _ = PyErr::fetch(py);
                return Err(PyErr::new::<RuntimeError, _>(e));
            }
        }
    }

    let result: &PyDict = match policy_results.len() {
        0 => PyDict::new(py),
        1 => policy_results
            .last()
            .clone()
            .expect("Failed to fetch policy result"),
        _ => policy_results.iter().fold(PyDict::new(py), |acc, a| {
            for (key, value) in a.iter() {
                match acc.get_item(key) {
                    Some(value_) => {
                        acc.set_item(
                            key,
                            value_
                                .call_method("__add__", (value,), None)
                                .expect("reduce_signals"),
                        )
                        .expect("reduce_signals");
                    }
                    None => {
                        acc.set_item(key, value).expect("reduce_signals");
                    }
                }
            }
            acc
        }),
    };
    Ok(result.into())
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn internal() {
//         assert!(true);
//     }
// }
