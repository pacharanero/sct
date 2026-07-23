// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PyO3 bindings for the local-first SNOMED CT SDK.

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::PyType;
use sct_rs::sdk::{SctError as RustSctError, SearchOptions, Snomed as RustSnomed, Terminology};
use serde::Serialize;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

create_exception!(_snomed_sct, SctError, PyException);
create_exception!(_snomed_sct, DatabaseError, SctError);
create_exception!(_snomed_sct, QueryError, SctError);
create_exception!(_snomed_sct, ValidationError, SctError);

fn python_error(error: RustSctError) -> PyErr {
    let message = error.to_string();
    match error {
        RustSctError::Open { .. }
        | RustSctError::UnsupportedSchema { .. }
        | RustSctError::InconsistentSchema { .. } => DatabaseError::new_err(message),
        RustSctError::InvalidSctid { .. }
        | RustSctError::UnsupportedTerminology { .. }
        | RustSctError::ConceptNotFound { .. } => ValidationError::new_err(message),
        _ => QueryError::new_err(message),
    }
}

fn json_to_python<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
    let json = serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("failed to encode result: {error}")))?;
    Ok(py.import("json")?.call_method1("loads", (json,))?.unbind())
}

fn with_session<T>(
    session: &Mutex<Option<RustSnomed>>,
    operation: impl FnOnce(&RustSnomed) -> Result<T, RustSctError>,
) -> PyResult<T> {
    let guard = session
        .lock()
        .map_err(|_| PyRuntimeError::new_err("SNOMED session lock is poisoned"))?;
    let snomed = guard
        .as_ref()
        .ok_or_else(|| PyRuntimeError::new_err("SNOMED session is closed"))?;
    operation(snomed).map_err(python_error)
}

fn parse_terminology(value: &str) -> PyResult<Terminology> {
    Terminology::from_str(value).map_err(python_error)
}

fn checked_limit(limit: i64) -> PyResult<u32> {
    u32::try_from(limit)
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| ValidationError::new_err("limit must be between 1 and 4294967295"))
}

/// A read-only SNOMED CT query session over an `sct sqlite` database.
#[pyclass(name = "Snomed", unsendable)]
struct PySnomed {
    path: PathBuf,
    session: Mutex<Option<RustSnomed>>,
}

#[pymethods]
impl PySnomed {
    #[new]
    fn new(path: PathBuf) -> PyResult<Self> {
        let session = RustSnomed::open(&path).map_err(python_error)?;
        Ok(Self {
            path,
            session: Mutex::new(Some(session)),
        })
    }

    #[getter]
    fn path(&self) -> String {
        self.path.display().to_string()
    }

    #[getter]
    fn closed(&self) -> PyResult<bool> {
        Ok(self
            .session
            .lock()
            .map_err(|_| PyRuntimeError::new_err("SNOMED session lock is poisoned"))?
            .is_none())
    }

    fn close(&self) -> PyResult<()> {
        self.session
            .lock()
            .map_err(|_| PyRuntimeError::new_err("SNOMED session lock is poisoned"))?
            .take();
        Ok(())
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        if slf.closed()? {
            return Err(PyRuntimeError::new_err("SNOMED session is closed"));
        }
        Ok(slf)
    }

    fn __exit__(
        &self,
        _exc_type: Option<&Bound<'_, PyType>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        self.close()
    }

    fn provenance(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let provenance = with_session(&self.session, |snomed| Ok(snomed.provenance().cloned()))?;
        provenance
            .as_ref()
            .map(|value| json_to_python(py, value))
            .transpose()
    }

    fn refsets(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let refsets = py.detach(|| with_session(&self.session, RustSnomed::refsets))?;
        json_to_python(py, &refsets)
    }

    fn refset(&self, py: Python<'_>, refset_id: &str) -> PyResult<Option<Py<PyAny>>> {
        let refset =
            py.detach(|| with_session(&self.session, |snomed| snomed.refset(refset_id)))?;
        refset
            .as_ref()
            .map(|value| json_to_python(py, value))
            .transpose()
    }

    #[pyo3(signature = (refset_id, limit=None))]
    fn refset_members(
        &self,
        py: Python<'_>,
        refset_id: &str,
        limit: Option<i64>,
    ) -> PyResult<Py<PyAny>> {
        let limit = limit.map(checked_limit).transpose()?;
        let members = py.detach(|| {
            with_session(&self.session, |snomed| {
                snomed.refset_members(refset_id, limit)
            })
        })?;
        json_to_python(py, &members)
    }

    fn history(&self, py: Python<'_>, concept_id: &str) -> PyResult<Py<PyAny>> {
        let history =
            py.detach(|| with_session(&self.session, |snomed| snomed.history(concept_id)))?;
        json_to_python(py, &history)
    }

    fn concept(&self, py: Python<'_>, concept_id: &str) -> PyResult<Option<Py<PyAny>>> {
        let concept =
            py.detach(|| with_session(&self.session, |snomed| snomed.concept(concept_id)))?;
        concept
            .as_ref()
            .map(|value| json_to_python(py, value))
            .transpose()
    }

    fn concepts(&self, py: Python<'_>, concept_ids: Vec<String>) -> PyResult<Py<PyAny>> {
        let concepts = py.detach(|| {
            with_session(&self.session, |snomed| {
                concept_ids
                    .iter()
                    .map(|id| snomed.concept(id))
                    .collect::<Result<Vec<_>, _>>()
            })
        })?;
        json_to_python(py, &concepts)
    }

    #[pyo3(signature = (query, limit=20, *, hierarchy=None, literal=false))]
    fn search(
        &self,
        py: Python<'_>,
        query: &str,
        limit: i64,
        hierarchy: Option<&str>,
        literal: bool,
    ) -> PyResult<Py<PyAny>> {
        let limit = checked_limit(limit)?;
        let hits = py.detach(|| {
            with_session(&self.session, |snomed| {
                let mut options = SearchOptions::new(query, limit);
                if let Some(hierarchy) = hierarchy {
                    options = options.hierarchy(hierarchy);
                }
                if literal {
                    options = options.literal();
                }
                snomed.search_with(options)
            })
        })?;
        json_to_python(py, &hits)
    }

    fn expand(&self, py: Python<'_>, expression: &str) -> PyResult<Vec<String>> {
        py.detach(|| with_session(&self.session, |snomed| snomed.expand(expression)))
    }

    #[pyo3(signature = (concept_id, limit=100))]
    fn children(&self, py: Python<'_>, concept_id: &str, limit: i64) -> PyResult<Py<PyAny>> {
        let limit = checked_limit(limit)?;
        let children =
            py.detach(|| with_session(&self.session, |snomed| snomed.children(concept_id, limit)))?;
        json_to_python(py, &children)
    }

    fn ancestors(&self, py: Python<'_>, concept_id: &str) -> PyResult<Py<PyAny>> {
        let ancestors =
            py.detach(|| with_session(&self.session, |snomed| snomed.ancestors(concept_id)))?;
        json_to_python(py, &ancestors)
    }

    #[pyo3(signature = (concept_id, limit=100))]
    fn descendants(&self, py: Python<'_>, concept_id: &str, limit: i64) -> PyResult<Py<PyAny>> {
        let limit = checked_limit(limit)?;
        let descendants = py.detach(|| {
            with_session(&self.session, |snomed| {
                snomed.descendants(concept_id, limit)
            })
        })?;
        json_to_python(py, &descendants)
    }

    fn subsumes(&self, py: Python<'_>, left: &str, right: &str) -> PyResult<String> {
        let relationship =
            py.detach(|| with_session(&self.session, |snomed| snomed.subsumes(left, right)))?;
        let value = serde_json::to_value(relationship).map_err(|error| {
            PyRuntimeError::new_err(format!("failed to encode result: {error}"))
        })?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }

    #[pyo3(signature = (source, code, target, *, forward_history=false))]
    fn map(
        &self,
        py: Python<'_>,
        source: &str,
        code: &str,
        target: &str,
        forward_history: bool,
    ) -> PyResult<Py<PyAny>> {
        let source = parse_terminology(source)?;
        let target = parse_terminology(target)?;
        let mappings = py.detach(|| {
            with_session(&self.session, |snomed| {
                if forward_history {
                    snomed.map_forwarding_history(source, code, target)
                } else {
                    snomed.map(source, code, target)
                }
            })
        })?;
        json_to_python(py, &mappings)
    }

    #[pyo3(signature = (source, codes, target, *, forward_history=false))]
    fn map_many(
        &self,
        py: Python<'_>,
        source: &str,
        codes: Vec<String>,
        target: &str,
        forward_history: bool,
    ) -> PyResult<Py<PyAny>> {
        let source = parse_terminology(source)?;
        let target = parse_terminology(target)?;
        let mappings = py.detach(|| {
            with_session(&self.session, |snomed| {
                codes
                    .iter()
                    .map(|code| {
                        if forward_history {
                            snomed.map_forwarding_history(source, code, target)
                        } else {
                            snomed.map(source, code, target)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
        })?;
        json_to_python(py, &mappings)
    }

    fn __repr__(&self) -> PyResult<String> {
        let state = if self.closed()? { "closed" } else { "open" };
        Ok(format!(
            "Snomed(path={:?}, state={state:?})",
            self.path.display().to_string()
        ))
    }
}

#[pymodule]
fn _snomed_sct(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PySnomed>()?;
    module.add("SctError", module.py().get_type::<SctError>())?;
    module.add("DatabaseError", module.py().get_type::<DatabaseError>())?;
    module.add("QueryError", module.py().get_type::<QueryError>())?;
    module.add("ValidationError", module.py().get_type::<ValidationError>())?;
    Ok(())
}
