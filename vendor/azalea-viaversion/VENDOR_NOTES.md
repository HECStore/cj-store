Vendored from `https://github.com/azalea-rs/azalea-viaversion` at
commit `c0d1e41f159a403fc2ea313f38cebb7fb4aaac44` (2026-04-08, "bump
viaproxy version (#18)") — current `main` HEAD at the time this was
copied in.

# Why a vendor copy and not a `git = ...` dep

Cargo treats two `git = "<URL>"` deps as the same source only if their
ref specs match exactly. Our parent `Cargo.toml` pins
`azalea = { git = "...", branch = "1.21.11" }` so the bot dodges the
26.1 protocol's broken ViaBackwards `container_set_content` translation.
Upstream `azalea-viaversion`'s `Cargo.toml` has plain
`azalea = { git = "..." }` (no ref). Cargo therefore creates two
distinct git sources and compiles two `azalea` instances side-by-side.
The build still succeeds — Bevy's plugin trait happens to unify across
the two — but the plugin's systems capture 26.1's ECS event types
(`ReceiveCustomQueryEvent`, `StartJoinServerEvent`, `Swarm`, ...) and
silently never fire on the 1.21.11 client's actually-emitted events,
breaking Mojang auth at runtime.

A `[patch."https://github.com/azalea-rs/azalea"]` section can't fix
this because cargo refuses to patch a git URL with itself
("patches must point to different sources"), and patching to the
crates.io release of the matching version is impossible because
`azalea 0.15.1+mc1.21.11` (the only published 1.21.11 release) has
exact pre-release pins (`signature = "=3.0.0-rc.8"`, `crypto-primes =
"=0.7.0-pre.7"`) whose transitive resolution against current
`rsa`/`pkcs8`/`der` pre-releases no longer compiles. The git branch
tip has refreshed those deps but never got a new crates.io release.

Vendoring lets us add `branch = "1.21.11"` to the local copy's
`azalea` dep so cargo unifies it with the parent's spec into a single
git source — one azalea instance, plugin systems hooked to the same
ECS world.

# What was changed vs upstream

`Cargo.toml`: added `branch = "1.21.11"` to the `azalea` dep. Nothing
else.

# When to re-sync

Drop this whole vendor dir and switch the parent `Cargo.toml` back to
`azalea-viaversion = { git = "..." }` once any of the following lands:

- ViaProxy publishes a release > 3.4.10 that bundles ViaBackwards >=
  5.9.0 (with the post-26.1 `custom_data` hashing fixes), AND
  `azalea-viaversion` bumps `VIA_PROXY_VERSION` to it. Then the 26.1
  azalea pin can stay, the diamond-chest decode no longer fails, and
  this whole pin tree becomes obsolete.
- An azalea 0.16.x (or later) release happens that azalea-viaversion's
  upstream `Cargo.toml` follows AND that fixes the 26.1
  `Protocol26_1To1_21_11` `container_set_content` rewriter on its own.
