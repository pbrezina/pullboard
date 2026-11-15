.DEFAULT_GOAL := image

CONTAINER_ENGINE ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)
IMAGE ?= localhost/pullboard:latest
CARGO ?= cargo
NPM ?= npm
MDBOOK ?= mdbook
CONFIG ?=
PORT ?= 3333
RUN_NAME ?=
SETUP_TARGET ?= x86_64-unknown-linux-musl
SETUP_BINARY ?= setup/target/$(SETUP_TARGET)/release/pullboard-setup
SETUP_DIST ?= target/setup-release

.PHONY: help image run smoke install build backend frontend setup package-setup dev dev-backend check-config test test-backend test-core test-setup test-frontend test-container test-scripts check-setup-dependencies e2e e2e-server e2e-install fmt check docs docs-serve

help:
	@echo 'make [image]  Build the local image (Podman preferred; CONTAINER_ENGINE=docker supported)'
	@echo 'make run      Run hardened on loopback; CONFIG=config/local.yaml optional, PORT=3333'
	@echo 'make smoke    Test an already-built image with a temporary loopback-only container'
	@echo 'make install  Install locked frontend dependencies'
	@echo 'make build    Build native backend and production frontend'
	@echo 'make dev      Start Angular dev server; run make dev-backend in another terminal'
	@echo 'make test/check/e2e/docs  Developer verification (see docs/src/development.md)'
	@echo 'make test-container  Test container mappings and Quadlet policy without starting services'
	@echo 'make setup/test-setup/test-core  Build static wizard or run Rust tests (locked)'
	@echo 'make test-scripts  Offline launcher, packaging, container and delivery policy tests'
	@echo 'make package-setup  Package an already-tested SETUP_BINARY; never rebuilds'
	@echo 'make e2e-server  Browser smoke against a real local Rust server and production Angular'

image:
	$(CONTAINER_ENGINE) build -t $(IMAGE) .

run:
	sh scripts/container-run.sh "$(CONTAINER_ENGINE)" "$(IMAGE)" "$(CONFIG)" "$(PORT)" "$(RUN_NAME)"

smoke:
	python3 scripts/container-smoke.py --engine "$(CONTAINER_ENGINE)" --image "$(IMAGE)"

install:
	cd frontend && $(NPM) ci

build: backend frontend

backend:
	$(CARGO) build --manifest-path backend/Cargo.toml --release --locked

frontend:
	cd frontend && $(NPM) run build

setup:
	$(CARGO) build --manifest-path setup/Cargo.toml --locked --release --target $(SETUP_TARGET)

package-setup:
	sh scripts/package-setup.sh "$(SETUP_BINARY)" "$(SETUP_DIST)"

dev:
	cd frontend && $(NPM) start

dev-backend:
	$(CARGO) run --manifest-path backend/Cargo.toml --locked -- --config "$(if $(CONFIG),$(CONFIG),config/pullboard.yaml)"

check-config:
	$(CARGO) run --manifest-path backend/Cargo.toml --locked -- --config "$(if $(CONFIG),$(CONFIG),config/pullboard.yaml)" --check-config

test: test-backend test-core test-setup test-frontend test-scripts

test-backend:
	$(CARGO) test --manifest-path backend/Cargo.toml --locked

test-core:
	$(CARGO) test --manifest-path crates/pullboard-core/Cargo.toml --locked

test-setup:
	$(CARGO) test --manifest-path setup/Cargo.toml --locked

check-setup-dependencies:
	$(CARGO) tree --manifest-path setup/Cargo.toml --locked --target $(SETUP_TARGET) --edges normal --prefix none --format '{p}' > "$(CURDIR)/target-setup-dependencies.log"
	python3 scripts/check-setup-dependencies.py < "$(CURDIR)/target-setup-dependencies.log"

test-frontend:
	cd frontend && $(NPM) test -- --watch=false

test-container:
	python3 -B -m unittest discover -s scripts/tests -p 'test_container.py' -v

test-scripts:
	python3 -B -m unittest discover -s scripts/tests -v

e2e-install:
	cd frontend && npx playwright install --with-deps chromium

e2e:
	cd frontend && $(NPM) run test:e2e -- --reporter=dot

e2e-server:
	$(CARGO) build --manifest-path backend/Cargo.toml --locked
	cd frontend && $(NPM) run test:server

fmt:
	$(CARGO) fmt --manifest-path backend/Cargo.toml
	$(CARGO) fmt --manifest-path crates/pullboard-core/Cargo.toml
	$(CARGO) fmt --manifest-path setup/Cargo.toml
	cd frontend && npx prettier --write src e2e playwright.config.ts

check:
	$(CARGO) fmt --manifest-path backend/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path backend/Cargo.toml --locked --all-targets -- -D warnings
	$(CARGO) fmt --manifest-path crates/pullboard-core/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path crates/pullboard-core/Cargo.toml --locked --all-targets -- -D warnings
	$(CARGO) fmt --manifest-path setup/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path setup/Cargo.toml --locked --all-targets -- -D warnings
	cd frontend && $(NPM) run typecheck
	cd frontend && $(NPM) run format:check

docs:
	$(MDBOOK) build docs

docs-serve:
	$(MDBOOK) serve docs --hostname 127.0.0.1
