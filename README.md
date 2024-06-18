# cargo watchdoc

[![CI Status](https://github.com/ModProg/cargo-watchdoc/actions/workflows/test.yaml/badge.svg)](https://github.com/ModProg/cargo-watchdoc/actions/workflows/test.yaml)
[![Crates.io](https://img.shields.io/crates/v/cargo-watchdoc)](https://crates.io/crates/cargo-watchdoc)

A CLI to live serve documentation for your crate while developing.

## Run
### General
Install via `cargo`: `cargo install cargo-watchdoc` and simply run as `cargo watchdoc` in your project.

### Nix
There's a `flake.nix` which also packaged `cargo-watchdoc`. You can simply run `nix run github:ModProg/cargo-watchdoc`.
