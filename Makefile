SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

HARNESS := ./scripts/cf-integration.sh

TOPOLOGY ?=
FRESH ?= 0
VOLUMES ?= 0
SERVICES ?=
USERS ?=
SPAWN_RATE ?=
RUN_TIME ?=
GROUP ?= all
LANES ?= fixture-direct controlplane dataplane
CLIENT_VERSION ?= 2026-07-28
SERVER_ERA ?= dual
RESULTS_DIR ?=
OUTPUT_DIR ?=
METHOD ?= tools/list
TOKEN_KIND ?= scoped
SERVER_ID ?=

.PHONY: help checkout up down reset status logs config probe load smoke \
	live live-mcp live-rbac live-protocol live-all conformance \
	conformance-report inspect token test

help: ## Show the Make command surface.
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-20s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

checkout: ## Clone or update configured control-plane/dataplane checkouts.
	@$(HARNESS) checkout

up: ## Start a topology. Use TOPOLOGY=controlplane|dataplane and FRESH=1.
	@TOPOLOGY="$(TOPOLOGY)" FRESH="$(FRESH)" $(HARNESS) up

down: ## Stop one or all topologies. Use TOPOLOGY=all and VOLUMES=1 as needed.
	@TOPOLOGY="$(TOPOLOGY)" VOLUMES="$(VOLUMES)" $(HARNESS) down

reset: ## Stop all topologies and remove their volumes.
	@TOPOLOGY="$(if $(TOPOLOGY),$(TOPOLOGY),all)" VOLUMES=1 $(HARNESS) down

status: ## Show Compose services for one topology.
	@TOPOLOGY="$(TOPOLOGY)" $(HARNESS) status

logs: ## Follow logs. Use SERVICES="nginx cf-dataplane".
	@TOPOLOGY="$(TOPOLOGY)" SERVICES="$(SERVICES)" $(HARNESS) logs

config: ## Render the merged Compose configuration.
	@TOPOLOGY="$(TOPOLOGY)" $(HARNESS) config

probe: ## Start, probe, and stop one public MCP topology.
	@TOPOLOGY="$(TOPOLOGY)" $(HARNESS) probe

load: ## Run Locust. Override USERS, SPAWN_RATE, and RUN_TIME.
	@TOPOLOGY="$(TOPOLOGY)" USERS="$(USERS)" SPAWN_RATE="$(SPAWN_RATE)" RUN_TIME="$(RUN_TIME)" $(HARNESS) load

smoke: ## Run a one-user, ten-second Locust smoke test.
	@TOPOLOGY="$(TOPOLOGY)" USERS="$(if $(USERS),$(USERS),1)" SPAWN_RATE="$(if $(SPAWN_RATE),$(SPAWN_RATE),1)" RUN_TIME="$(if $(RUN_TIME),$(RUN_TIME),10s)" $(HARNESS) load

live: ## Run an upstream live group. Use GROUP=mcp|rbac|protocol|all.
	@TOPOLOGY="$(TOPOLOGY)" GROUP="$(GROUP)" $(HARNESS) live

live-mcp: ## Run upstream MCP protocol E2E tests.
	@TOPOLOGY="$(TOPOLOGY)" GROUP=mcp $(HARNESS) live

live-rbac: ## Run upstream MCP RBAC/multi-transport tests.
	@TOPOLOGY="$(TOPOLOGY)" GROUP=rbac $(HARNESS) live

live-protocol: ## Run upstream gateway protocol-compliance tests.
	@TOPOLOGY="$(TOPOLOGY)" GROUP=protocol $(HARNESS) live

live-all: ## Run the complete upstream live-gateway suite.
	@TOPOLOGY="$(TOPOLOGY)" GROUP=all $(HARNESS) live

conformance: ## Run official MCP conformance lanes.
	@TOPOLOGY="$(TOPOLOGY)" LANES="$(LANES)" CLIENT_VERSION="$(CLIENT_VERSION)" SERVER_ERA="$(SERVER_ERA)" RESULTS_DIR="$(RESULTS_DIR)" $(HARNESS) conformance

conformance-report: ## Rebuild the comparison report from existing artifacts.
	@RESULTS_DIR="$(RESULTS_DIR)" OUTPUT_DIR="$(OUTPUT_DIR)" $(HARNESS) conformance-report

inspect: ## Run the official MCP Inspector CLI against one topology.
	@TOPOLOGY="$(TOPOLOGY)" METHOD="$(METHOD)" SERVER_ID="$(SERVER_ID)" $(HARNESS) inspect

token: ## Print a token. Use TOKEN_KIND=scoped|admin and SERVER_ID as needed.
	@TOKEN_KIND="$(TOKEN_KIND)" SERVER_ID="$(SERVER_ID)" $(HARNESS) token

test: ## Run shell syntax and Python unit tests without Docker mutations.
	@bash -n scripts/cf-integration.sh
	@python3 -m unittest discover -s tests -p 'test_*.py'
