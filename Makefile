# Makefile compartilhado dos `rust_py_*`.
#
# Existe para que a CI e a pessoa rodem exatamente o mesmo comando. Quando o
# lint da CI diverge do lint local, quem perde é sempre quem abriu o PR.
#
# A toolchain NÃO é fixada por `rust-toolchain.toml` de propósito: esse arquivo
# vale para toda invocação do cargo, inclusive a dos containers manylinux do
# release, e um download de toolchain no meio de uma publicação no PyPI é risco
# sem contrapartida. A fixação vive no job de lint da CI, que é onde
# `-D warnings` realmente morde. Localmente, use uma stable recente.

PYTHON ?= python3

# Fixa qual Python o pyo3 usa na compilação. Sem isto ele pega o primeiro do
# PATH, que pode não ser o mesmo do `make dev` — e aí o binário de teste linka
# contra uma libpython e o pytest roda contra outra.
export PYO3_PYTHON ?= $(PYTHON)

# No macOS o binário de `cargo test` carrega a libpython em runtime, e o dyld
# não a encontra sozinho: o erro é `Library not loaded: @rpath/libpython3.X.dylib`,
# em SIGABRT, depois de compilar com sucesso — parece falha de teste e não é.
# Os caminhos padrão entram junto porque definir DYLD_FALLBACK_LIBRARY_PATH
# *substitui* a lista default do dyld em vez de acrescentar a ela (senão o
# erro seguinte é libiconv.2.dylib).
PY_LIBDIR := $(shell $(PYTHON) -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR") or "")' 2>/dev/null)
export DYLD_FALLBACK_LIBRARY_PATH := $(PY_LIBDIR):/opt/homebrew/lib:/usr/local/lib:/usr/lib

.PHONY: help dev test lint fmt clippy test-rust test-python clean

help:
	@echo "make dev          instala o pacote em modo editável com os extras de dev"
	@echo "make test         cargo test + pytest"
	@echo "make lint         cargo fmt --check + cargo clippy -D warnings"
	@echo "make fmt          aplica a formatação (escreve nos arquivos)"

# `pip install` aciona o backend maturin, compila a extensão e instala.
# Ao contrário de `maturin develop`, não exige um virtualenv ativo — que é
# exatamente a armadilha que custou um release do monitor (ver P1-9).
dev:
	$(PYTHON) -m pip install --upgrade pip
	$(PYTHON) -m pip install -e ".[dev]"

test: test-rust test-python

# --no-default-features desliga `extension-module`, que impede o binário de
# teste de linkar contra a libpython no Linux. --lib porque os testes de
# integração precisam do módulo nativo, e esses ficam do lado Python.
test-rust:
	cargo test --no-default-features --lib

test-python:
	pytest -q

lint: fmt-check clippy

.PHONY: fmt-check
fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

clean:
	cargo clean
	rm -rf dist build *.egg-info .pytest_cache
