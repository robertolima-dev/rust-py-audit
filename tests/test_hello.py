import rust_py_audit


def test_hello_returns_a_string():
    result = rust_py_audit.hello()

    assert isinstance(result, str)
    assert len(result) > 0
