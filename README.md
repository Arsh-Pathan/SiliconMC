# SiliconMC

A clean-room **Rust** Minecraft **26.3** (`protocol_version: 777`, `world_version: 5023`) server implementation built for exact vanilla parity and verified via live differential testing against an official Java 26.3 server runtime.

> **Copyright Notice:** This repository contains **zero** Minecraft or Mojang source code, assets, datapacks, JAR files, or derived game data tables. All Minecraft-derived protocol/worldgen tables are generated locally from your own copy of the official Minecraft 26.3 `server.jar`.

## Architecture

SiliconMC is organized as an 8-crate Rust 2024 Cargo workspace under `rust/`:

| Crate | Responsibility |
| --- | --- |
| `mc-math` | Bit-exact `Mth` LUT trigonometry, `BlockPos`/`ChunkPos`/`Vec3`, `LegacyRandomSource`, and `Xoroshiro128++` RNG |
| `mc-nbt` | Full file & network NBT codec with Java Modified UTF-8 (`MUTF-8`) support |
| `mc-oracle` | Persistent NDJSON IPC bridge to `OracleMain` for automated differential tests |
| `mc-data` | Compile-time registry/packet/block-state tables, datapack loader, recipes, fuels, foods, loot, and combat stats |
| `mc-world` | Generational `Arena<T>`, Anvil `.mca` region I/O, chunk palettes, BFS skylight/blocklight, tick scheduler, redstone/pistons, fluids, containers, block entities, entities, and `.nbt` `StructureTemplate` loader |
| `mc-worldgen` | Bit-exact Perlin/Normal/Blended/Simplex noise, 6D `ClimateRTree`, `NoiseBasedAquifer`, `MaterialSystemState`, Overworld/Nether/End generators, and ore/tree/flora decoration |
| `mc-proto` | Framing, zlib compression, AES-CFB8 encryption, Mojang auth digest, and Protocol 777 codecs |
| `mc-server` | Deterministic 20 TPS simulation loop + async `tokio` networking, multi-dimension `WorldStore`, commands, survival/combat/food, admin lists, Source RCON, and Minecraft 26.3 JSON-RPC 2.0 Management Server |

## Local Setup & Data Generation

Because Mojang's game files are not distributed in this repository, generate the local data tables once using your own copy of the official Minecraft 26.3 `server.jar`:

1. Place your official Minecraft 26.3 `server.jar` in the repository root and extract Minecraft's reports:
   ```bash
   java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --output generated
   ```
2. Extract the vanilla `data/minecraft/` datapack directory into `src/main/resources/data/minecraft/`.
3. Export `generated/reports/runtime_tables.json` and `rust/fixtures/vanilla_network_tags.bin` via `com.siliconmc.oracle.OracleMain` (compiled locally against `server.jar`).
4. Run the Python codegen pipeline to build the local Rust tables:
   ```bash
   python scripts/generate_all.py
   ```

## Building & Testing

```bash
# Run the full test suite (including live differential oracle & E2E client tests)
cargo test --manifest-path rust/Cargo.toml --workspace

# Build an optimized release server binary
cargo build --release --manifest-path rust/Cargo.toml -p mc-server
```
