.DEFAULT_GOAL := image

CONTAINER_ENGINE ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)
IMAGE ?= localhost/pullboard:latest
CARGO ?= cargo
NPM ?= npm
MDBOOK ?= mdbook
CONFIG ?=
PORT ?= 8080
RUN_NAME ?=

.PHONY: help image run smoke install build backend frontend dev dev-backend check-config test test-backend test-frontend e2e e2e-server e2e-install fmt check docs docs-serve

help:
	@echo 'make [image]  Build the local image (Podman preferred; CONTAINER_ENGINE=docker supported)'
	@echo 'make run      Run hardened on loopback; CONFIG=config/local.yaml optional, PORT=8080'
	@echo 'make smoke    Test an already-built image with a temporary loopback-only container'
	@echo 'make install  Install locked frontend dependencies'
	@echo 'make build    Build native backend and production frontend'
	@echo 'make dev      Start Angular dev server; run make dev-backend in another terminal'
	@echo 'make test/check/e2e/docs  Developer verification (see docs/src/development.md)'
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

dev:
	cd frontend && $(NPM) start

dev-backend:
	$(CARGO) run --manifest-path backend/Cargo.toml --locked -- --config "$(if $(CONFIG),$(CONFIG),config/pullboard.yaml)"

check-config:
	$(CARGO) run --manifest-path backend/Cargo.toml --locked -- --config "$(if $(CONFIG),$(CONFIG),config/pullboard.yaml)" --check-config

test: test-backend test-frontend

test-backend:
	$(CARGO) test --manifest-path backend/Cargo.toml --locked

test-frontend:
	cd frontend && $(NPM) test -- --watch=false

e2e-install:
	cd frontend && npx playwright install --with-deps chromium

e2e:
	cd frontend && $(NPM) run test:e2e -- --reporter=dot

e2e-server:
	$(CARGO) build --manifest-path backend/Cargo.toml --locked
	cd frontend && $(NPM) run test:server

fmt:
	$(CARGO) fmt --manifest-path backend/Cargo.toml
	cd frontend && npx prettier --write src e2e playwright.config.ts

check:
	$(CARGO) fmt --manifest-path backend/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path backend/Cargo.toml --locked --all-targets -- -D warnings
	cd frontend && $(NPM) run typecheck
	cd frontend && $(NPM) run format:check

docs:
	$(MDBOOK) build docs

docs-serve:
	$(MDBOOK) serve docs --hostname 127.0.0.1
