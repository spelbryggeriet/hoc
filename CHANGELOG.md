# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Image tag: ghcr.io/spelbryggeriet/game-box-backend:0.0.6

## [0.0.6] - 2023-01-03

### Added

- The `deploy` command will now read a certain file (called `hocfile.yaml`), which contains
  information on the application and how to deploy it, and use that to deploy it as a Helm chart in
  the Kubernetes cluster.

## [0.0.5] - 2022-12-26

Image tag: ghcr.io/spelbryggeriet/game-box-backend:0.0.5

### Changed

- Processes (system commands) are now run in a container (using Docker) by default. This requires
  that Docker is installed on the host.

## [0.0.4] - 2022-12-19

### Changed

- The `sd-card-prepare` command now modifies the flashed SD card with
  [cloud-init](https://cloud-init.io) settings.
- The `node-deploy` command now deploys a node into the cluster. Currently, only a single node
  cluster is supported.

### Fixed

- The `upgrade` command will now work properly with the `--from-ref` flag when a branch has been
  fore-pushed with new commits.
- The `upgrade` command will now check if an SD card has previously been flashed.
- An issue where some logs would not be written to the `~/.local/share/hoc/logs` folder.

## [0.0.3] - 2022-12-01

### Added

- A `version` command to show the current version of `hoc`.

## [0.0.2] - 2022-11-30

### Added

- An `upgrade` command to upgrade `hoc` itself.

### Changed

- Shell command output is now less verbose.

## [0.0.1] - 2022-11-26

### Added

- A command for flashing an SD card with the Ubuntu OS.
- A terminal aware logging framework, also writing logs to file in the back end.
- Support for user-input via prompts.
- The ability to revert changes if a failure occurs.
