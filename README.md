# HomePi Hosting Tools
The purpose of this tool is to help deploy clusterizd HomePi applications to a Raspberry Pi kubernetes cluster.

## Compiling
In order to compile a development version of the project, run the following:

```bash
cargo build
```

To get a release build, run:

```bash
cargo build --release
```

## Configuring Mac Pro Registry server
When setting up the Mac, set an appropriate string password and allow SSH access
in the System Preferences Sharing pane. Then login remotely from a client
computer.

### SSH setup
Set hostname:

```bash
sudo scutil --set ComputerName HomePi-Registry
sudo scutil --set LocalHostName homepi-registry
sudo scutil --set HostName homepi-registry
```
