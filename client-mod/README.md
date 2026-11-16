Paper-RS Boost
==============

An optional Fabric mod for Minecraft 26.3 clients. On a Paper-RS server that allows it
(`boost-mod = true` under `[network]` in the server's `config/paper-rs.toml`), the client
receives its chunks over a few extra connections: each is written and encrypted by its own
server thread and read, decrypted and decoded by its own client thread, so a player's world
data no longer queues behind everything else on one connection. Markers on the main
connection keep every chunk in its place among the other packets.

On other servers, or when the server leaves it off, the mod does nothing.

Building needs JDK 25 (the first build downloads Gradle, Fabric Loom, Fabric API and
Minecraft 26.3 to compile against):

```bash
./gradlew build
```

Install `build/libs/paper-rs-boost-1.0.0.jar` with Fabric Loader and Fabric API.

The protocol is described in `crates/paper-server/src/net/boost.rs`. Licensed like the rest
of Paper-RS (GPL-3.0-or-later).
