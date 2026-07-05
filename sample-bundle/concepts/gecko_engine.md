---
type: concept
title: GECKO Engine Core
description: The foundational runtime and execution orchestrator for the GECKO framework.
tags: [core, engine, architecture]
---

# GECKO Engine Core

The `gecko-engine` is the heart of the framework. It handles the ingestion of Open Knowledge Format (OKF) bundles, synchronizes them into TypeDB to construct the Knowledge Graph, and orchestrates the execution of playbooks in a sandboxed environment.

It relies on a polymorphic extensions model (via the `GeckoExtension` trait) to dynamically inject domain-specific capabilities into the engine, allowing it to adapt to multiple domains like cybersecurity or memory management without changing its core source.
