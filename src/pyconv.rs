//! Conversões entre objetos Python e `serde_json::Value`.
//!
//! `metadata` é "qualquer JSON válido" do lado Rust, mas do lado Python
//! ele chega como um `dict` comum (com listas, strings, números, etc.
//! dentro). Este módulo faz a ponte nos dois sentidos: Python -> JSON
//! (para guardar/hashear o evento) e JSON -> Python (para devolver o
//! evento como `dict` em `log()`/`verify()`).
//!
//! Por que não usar uma crate como `pythonize` para isso? Para manter a
//! lista de dependências enxuta e restrita ao que foi definido para o
//! projeto (pyo3, serde, serde_json, sha2, uuid, time) — a conversão
//! manual também deixa explícito exatamente quais tipos Python são
//! suportados em `metadata`.

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

use crate::event::AuditEvent;

/// Converte um objeto Python qualquer em `serde_json::Value`.
///
/// É recursiva: uma lista/dict pode conter outras listas/dicts dentro.
/// A ordem dos `if let` importa — em Python, `bool` é uma subclasse de
/// `int` (`isinstance(True, int)` é `True`!), então precisamos checar
/// `PyBool` *antes* de `PyInt`, ou todo `True`/`False` em metadata
/// viraria silenciosamente `1`/`0`.
pub fn python_to_json(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if value.is_none() {
        return Ok(serde_json::Value::Null);
    }

    if let Ok(boolean) = value.downcast::<PyBool>() {
        return Ok(serde_json::Value::Bool(boolean.is_true()));
    }

    if let Ok(integer) = value.downcast::<PyInt>() {
        let number: i64 = integer.extract()?;
        return Ok(serde_json::Value::from(number));
    }

    if let Ok(float) = value.downcast::<PyFloat>() {
        let number: f64 = float.extract()?;
        return serde_json::Number::from_f64(number)
            .map(serde_json::Value::Number)
            .ok_or_else(|| {
                PyTypeError::new_err("valor de ponto flutuante inválido (NaN/Infinity) em metadata")
            });
    }

    if let Ok(text) = value.downcast::<PyString>() {
        return Ok(serde_json::Value::String(text.to_string()));
    }

    if let Ok(list) = value.downcast::<PyList>() {
        let items = list
            .iter()
            .map(|item| python_to_json(&item))
            .collect::<PyResult<Vec<_>>>()?;
        return Ok(serde_json::Value::Array(items));
    }

    if let Ok(tuple) = value.downcast::<PyTuple>() {
        let items = tuple
            .iter()
            .map(|item| python_to_json(&item))
            .collect::<PyResult<Vec<_>>>()?;
        return Ok(serde_json::Value::Array(items));
    }

    if let Ok(dict) = value.downcast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (key, val) in dict.iter() {
            let key: String = key
                .extract()
                .map_err(|_| PyTypeError::new_err("chaves de metadata precisam ser strings"))?;
            map.insert(key, python_to_json(&val)?);
        }
        return Ok(serde_json::Value::Object(map));
    }

    Err(PyTypeError::new_err(
        "tipo não suportado em metadata (use None, bool, int, float, str, list, tuple ou dict)",
    ))
}

/// Converte um `serde_json::Value` de volta para um objeto Python.
/// É o caminho inverso de `python_to_json`, usado para devolver
/// `metadata` (e o evento inteiro) como estruturas Python nativas.
pub fn json_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<PyObject> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(boolean) => Ok(boolean.into_py(py)),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Ok(integer.into_py(py))
            } else if let Some(float) = number.as_f64() {
                Ok(float.into_py(py))
            } else {
                Err(PyTypeError::new_err("número inválido em metadata"))
            }
        }
        serde_json::Value::String(text) => Ok(text.into_py(py)),
        serde_json::Value::Array(items) => {
            let list = PyList::empty_bound(py);
            for item in items {
                list.append(json_to_python(py, item)?)?;
            }
            Ok(list.into())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new_bound(py);
            for (key, val) in map {
                dict.set_item(key, json_to_python(py, val)?)?;
            }
            Ok(dict.into())
        }
    }
}

/// Converte um `AuditEvent` inteiro para o `dict` Python que `log()` e
/// `verify()` devolvem ao chamador.
pub fn event_to_pydict(py: Python<'_>, event: &AuditEvent) -> PyResult<PyObject> {
    let dict = PyDict::new_bound(py);
    dict.set_item("id", &event.id)?;
    dict.set_item("timestamp", &event.timestamp)?;
    dict.set_item("app_name", &event.app_name)?;
    dict.set_item("actor_id", &event.actor_id)?;
    dict.set_item("action", &event.action)?;
    dict.set_item("resource", &event.resource)?;
    dict.set_item("resource_id", &event.resource_id)?;
    dict.set_item("metadata", json_to_python(py, &event.metadata)?)?;
    // `Option<&str>`: o PyO3 converte `None` para o `None` do Python e
    // `Some(texto)` para uma `str` normal — não precisamos de um `match`
    // manual aqui.
    dict.set_item("previous_hash", event.previous_hash.as_deref())?;
    dict.set_item("hash", &event.hash)?;
    Ok(dict.into())
}
