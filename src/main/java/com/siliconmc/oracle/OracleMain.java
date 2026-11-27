package com.siliconmc.oracle;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import java.io.BufferedReader;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Base64;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.LayeredRegistryAccess;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.RegistrySynchronization;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.nbt.Tag;
import net.minecraft.nbt.TagParser;
import net.minecraft.network.PacketListener;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.configuration.ConfigurationProtocols;
import net.minecraft.network.protocol.game.GameProtocols;
import net.minecraft.network.protocol.handshake.HandshakeProtocols;
import net.minecraft.network.protocol.login.LoginProtocols;
import net.minecraft.network.protocol.status.StatusProtocols;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.RegistryOps;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.packs.PackResources;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.KnownPack;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagLoader;
import net.minecraft.tags.TagNetworkSerialization;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.util.Util;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

/**
 * Persistent NDJSON stdin/stdout differential testing oracle backed by Minecraft 26.3.
 */
public final class OracleMain {
    private static final Gson GSON = new GsonBuilder().disableHtmlEscaping().create();
    private static volatile LayeredRegistryAccess<RegistryLayer> cachedLayeredRegistries;
    private static volatile ProtocolInfo<?> cachedPlayServerbound;
    private static volatile ProtocolInfo<?> cachedPlayClientbound;

    public static void main(String[] args) throws Exception {
        PrintStream rpcOut = System.out;
        System.setOut(System.err);

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        BufferedReader reader = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        String line;
        while ((line = reader.readLine()) != null) {
            line = line.trim();
            if (line.isEmpty()) {
                continue;
            }
            JsonObject response;
            boolean shouldQuit = false;
            try {
                JsonObject req = JsonParser.parseString(line).getAsJsonObject();
                String op = req.get("op").getAsString();
                if ("quit".equals(op)) {
                    response = ok();
                    shouldQuit = true;
                } else {
                    response = dispatch(op, req);
                }
            } catch (Throwable t) {
                response = new JsonObject();
                response.addProperty("ok", false);
                response.addProperty("error", t.getClass().getName() + ": " + t.getMessage());
            }
            rpcOut.println(GSON.toJson(response));
            rpcOut.flush();
            if (shouldQuit) {
                break;
            }
        }
        Util.shutdownExecutors();
    }

    private static JsonObject ok() {
        JsonObject o = new JsonObject();
        o.addProperty("ok", true);
        return o;
    }

    private static synchronized LayeredRegistryAccess<RegistryLayer> getLayeredRegistries() {
        if (cachedLayeredRegistries == null) {
            try (MultiPackResourceManager rm = new MultiPackResourceManager(
                PackType.SERVER_DATA,
                List.<PackResources>of(ServerPacksSource.createVanillaPackSource().fullResources())
            )) {
                LayeredRegistryAccess<RegistryLayer> initialLayers = RegistryLayer.createRegistryAccess();
                List<Registry.PendingTags<?>> staticLayerTags = TagLoader.loadTagsForExistingRegistries(
                    rm, initialLayers.getLayer(RegistryLayer.STATIC)
                );
                RegistryAccess.Frozen worldLoadContext = initialLayers.getAccessForLoading(RegistryLayer.WORLD);
                List<HolderLookup.RegistryLookup<?>> worldContextRegistries = TagLoader.buildUpdatedLookups(worldLoadContext, staticLayerTags);
                RegistryAccess.Frozen loadedWorldRegistries = RegistryDataLoader.load(
                    rm,
                    worldContextRegistries,
                    RegistryDataLoader.WORLD_REGISTRIES,
                    Util.backgroundExecutor()
                ).join();
                staticLayerTags.forEach(Registry.PendingTags::apply);
                cachedLayeredRegistries = initialLayers.replaceFrom(RegistryLayer.WORLD, loadedWorldRegistries);
                BuiltInRegistries.DATA_COMPONENT_INITIALIZERS
                    .build(cachedLayeredRegistries.compositeAccess())
                    .forEach(net.minecraft.core.component.DataComponentInitializers.PendingComponents::apply);
            }
        }
        return cachedLayeredRegistries;
    }

    private static RegistryAccess.Frozen getFullRegistries() {
        return getLayeredRegistries().compositeAccess();
    }

    private static synchronized ProtocolInfo<?> getPlayProtocol(boolean serverbound) {
        if (cachedPlayServerbound == null || cachedPlayClientbound == null) {
            RegistryAccess.Frozen regs = getFullRegistries();
            GameProtocols.Context ctx = new GameProtocols.Context() {
                @Override
                public boolean hasInfiniteMaterials() {
                    return true;
                }

                @Override
                public boolean canUseCommandBlocks() {
                    return true;
                }
            };
            cachedPlayServerbound = GameProtocols.SERVERBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(regs), ctx);
            cachedPlayClientbound = GameProtocols.CLIENTBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(regs));
        }
        return serverbound ? cachedPlayServerbound : cachedPlayClientbound;
    }

    private static JsonObject dispatch(String op, JsonObject req) throws Exception {
        return switch (op) {
            case "ping" -> handlePing();
            case "mth_sample" -> handleMthSample(req);
            case "rng_legacy" -> handleRng(req, false);
            case "rng_xoroshiro" -> handleRng(req, true);
            case "nbt_roundtrip" -> handleNbtRoundtrip(req);
            case "nbt_from_snbt" -> handleNbtFromSnbt(req);
            case "mca_read_chunk" -> handleMcaReadChunk(req);
            case "packet_roundtrip" -> handlePacketRoundtrip(req);
            case "dump_synced_registries" -> handleDumpSyncedRegistries(req);
            case "dump_network_tags" -> handleDumpNetworkTags();
            case "dump_runtime_tables" -> handleDumpRuntimeTables(req);
            case "noise_sample" -> handleNoiseSample(req);
            case "climate_sample" -> handleClimateSample(req);
            case "noise_router_sample" -> handleNoiseRouterSample(req);
            default -> throw new IllegalArgumentException("Unknown op: " + op);
        };
    }

    private static JsonObject handlePing() {
        JsonObject res = ok();
        res.addProperty("version", SharedConstants.getCurrentVersion().name());
        res.addProperty("protocol", SharedConstants.getProtocolVersion());
        res.addProperty("world_version", SharedConstants.getCurrentVersion().dataVersion().version());
        return res;
    }

    private static JsonObject handleMthSample(JsonObject req) {
        JsonObject res = ok();
        if (req.has("trig_inputs")) {
            JsonArray sinBits = new JsonArray();
            JsonArray cosBits = new JsonArray();
            for (JsonElement el : req.getAsJsonArray("trig_inputs")) {
                double v = Double.longBitsToDouble(el.getAsLong());
                sinBits.add(Float.floatToRawIntBits(Mth.sin(v)));
                cosBits.add(Float.floatToRawIntBits(Mth.cos(v)));
            }
            res.add("sin_bits", sinBits);
            res.add("cos_bits", cosBits);
        }
        if (req.has("block_positions")) {
            JsonArray packed = new JsonArray();
            JsonArray unpacked = new JsonArray();
            JsonArray seeds = new JsonArray();
            for (JsonElement el : req.getAsJsonArray("block_positions")) {
                JsonArray arr = el.getAsJsonArray();
                int x = arr.get(0).getAsInt();
                int y = arr.get(1).getAsInt();
                int z = arr.get(2).getAsInt();
                long p = BlockPos.asLong(x, y, z);
                packed.add(p);
                JsonArray u = new JsonArray();
                u.add(BlockPos.getX(p));
                u.add(BlockPos.getY(p));
                u.add(BlockPos.getZ(p));
                unpacked.add(u);
                seeds.add(Mth.getSeed(x, y, z));
            }
            res.add("packed", packed);
            res.add("unpacked", unpacked);
            res.add("seeds", seeds);
        }
        return res;
    }

    private static JsonObject handleRng(JsonObject req, boolean xoroshiro) {
        RandomSource rng;
        if (xoroshiro && req.has("seed_lo") && req.has("seed_hi")) {
            rng = new XoroshiroRandomSource(req.get("seed_lo").getAsLong(), req.get("seed_hi").getAsLong());
        } else {
            long seed = req.get("seed").getAsLong();
            rng = xoroshiro ? new XoroshiroRandomSource(seed) : new LegacyRandomSource(seed);
        }

        if (req.has("positional_at")) {
            JsonArray pos = req.getAsJsonArray("positional_at");
            PositionalRandomFactory factory = rng.forkPositional();
            rng = factory.at(pos.get(0).getAsInt(), pos.get(1).getAsInt(), pos.get(2).getAsInt());
        } else if (req.has("positional_hash")) {
            PositionalRandomFactory factory = rng.forkPositional();
            rng = factory.fromHashOf(req.get("positional_hash").getAsString());
        }

        int count = req.has("count") ? req.get("count").getAsInt() : 16;
        JsonArray ints = new JsonArray();
        JsonArray boundedInts = new JsonArray();
        JsonArray longs = new JsonArray();
        JsonArray booleans = new JsonArray();
        JsonArray floatBits = new JsonArray();
        JsonArray doubleBits = new JsonArray();
        JsonArray gaussianBits = new JsonArray();

        int[] bounds = new int[]{1, 2, 3, 7, 16, 17, 100, 1024, 1_000_003};
        for (int i = 0; i < count; i++) {
            ints.add(rng.nextInt());
            boundedInts.add(rng.nextInt(bounds[i % bounds.length]));
            longs.add(rng.nextLong());
            booleans.add(rng.nextBoolean());
            floatBits.add(Float.floatToRawIntBits(rng.nextFloat()));
            doubleBits.add(Double.doubleToRawLongBits(rng.nextDouble()));
            gaussianBits.add(Double.doubleToRawLongBits(rng.nextGaussian()));
        }

        JsonObject res = ok();
        res.add("ints", ints);
        res.add("bounded_ints", boundedInts);
        res.add("longs", longs);
        res.add("booleans", booleans);
        res.add("float_bits", floatBits);
        res.add("double_bits", doubleBits);
        res.add("gaussian_bits", gaussianBits);
        return res;
    }

    private static JsonObject handleNbtRoundtrip(JsonObject req) throws Exception {
        String mode = req.has("mode") ? req.get("mode").getAsString() : "file";
        byte[] raw = Base64.getDecoder().decode(req.get("bytes_b64").getAsString());
        ByteArrayInputStream bais = new ByteArrayInputStream(raw);
        DataInputStream dis = new DataInputStream(bais);
        Tag tag;
        if ("network".equals(mode)) {
            tag = NbtIo.readAnyTag(dis, NbtAccounter.unlimitedHeap());
        } else {
            tag = NbtIo.read(dis, NbtAccounter.unlimitedHeap());
        }
        if (bais.available() != 0) {
            throw new IllegalArgumentException("Trailing bytes after NBT decode: " + bais.available());
        }
        ByteArrayOutputStream baos = new ByteArrayOutputStream();
        DataOutputStream dos = new DataOutputStream(baos);
        if ("network".equals(mode)) {
            NbtIo.writeAnyTag(tag, dos);
        } else {
            NbtIo.write((CompoundTag) tag, dos);
        }
        dos.flush();

        JsonObject res = ok();
        res.addProperty("bytes_b64", Base64.getEncoder().encodeToString(baos.toByteArray()));
        res.addProperty("snbt", tag.toString());
        return res;
    }

    private static JsonObject handleNbtFromSnbt(JsonObject req) throws Exception {
        String snbt = req.get("snbt").getAsString();
        String mode = req.has("mode") ? req.get("mode").getAsString() : "file";
        CompoundTag tag = TagParser.parseCompoundFully(snbt);
        ByteArrayOutputStream baos = new ByteArrayOutputStream();
        DataOutputStream dos = new DataOutputStream(baos);
        if ("network".equals(mode)) {
            NbtIo.writeAnyTag(tag, dos);
        } else {
            NbtIo.write(tag, dos);
        }
        dos.flush();

        JsonObject res = ok();
        res.addProperty("bytes_b64", Base64.getEncoder().encodeToString(baos.toByteArray()));
        res.addProperty("snbt", tag.toString());
        return res;
    }

    private static JsonObject handleMcaReadChunk(JsonObject req) throws Exception {
        Path path = Path.of(req.get("path").getAsString());
        int cx = req.get("cx").getAsInt();
        int cz = req.get("cz").getAsInt();
        RegionStorageInfo info = new RegionStorageInfo("oracle", Level.OVERWORLD, "chunk");
        try (RegionFile rf = new RegionFile(info, path, path.getParent(), false)) {
            DataInputStream dis = rf.getChunkDataInputStream(new ChunkPos(cx, cz));
            JsonObject res = ok();
            if (dis == null) {
                res.addProperty("present", false);
                return res;
            }
            CompoundTag tag;
            try (dis) {
                tag = NbtIo.read(dis, NbtAccounter.unlimitedHeap());
            }
            ByteArrayOutputStream baos = new ByteArrayOutputStream();
            try (DataOutputStream dos = new DataOutputStream(baos)) {
                NbtIo.write(tag, dos);
            }
            res.addProperty("present", true);
            res.addProperty("bytes_b64", Base64.getEncoder().encodeToString(baos.toByteArray()));
            res.addProperty("data_version", tag.getIntOr("DataVersion", -1));
            res.addProperty("status", tag.getStringOr("Status", ""));
            return res;
        }
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static JsonObject handlePacketRoundtrip(JsonObject req) {
        String phase = req.get("phase").getAsString();
        String direction = req.get("direction").getAsString();
        byte[] raw = Base64.getDecoder().decode(req.get("bytes_b64").getAsString());

        boolean serverbound = "serverbound".equals(direction);
        ProtocolInfo<?> info = switch (phase) {
            case "handshake" -> {
                if (!serverbound) throw new IllegalArgumentException("Handshake has only serverbound packets");
                yield HandshakeProtocols.SERVERBOUND;
            }
            case "status" -> serverbound ? StatusProtocols.SERVERBOUND : StatusProtocols.CLIENTBOUND;
            case "login" -> serverbound ? LoginProtocols.SERVERBOUND : LoginProtocols.CLIENTBOUND;
            case "configuration" -> serverbound ? ConfigurationProtocols.SERVERBOUND : ConfigurationProtocols.CLIENTBOUND;
            case "play" -> getPlayProtocol(serverbound);
            default -> throw new IllegalArgumentException("Unknown phase: " + phase);
        };

        StreamCodec<ByteBuf, Packet<?>> codec = (StreamCodec) info.codec();
        ByteBuf inBuf = Unpooled.wrappedBuffer(raw);
        Packet<?> packet;
        try {
            packet = codec.decode(inBuf);
            if (inBuf.readableBytes() != 0) {
                throw new IllegalArgumentException(
                    "Trailing bytes after decoding " + packet.type().id() + ": " + inBuf.readableBytes() + " remaining"
                );
            }
        } finally {
            inBuf.release();
        }

        ByteBuf outBuf = Unpooled.buffer();
        byte[] reencoded;
        try {
            codec.encode(outBuf, packet);
            reencoded = new byte[outBuf.readableBytes()];
            outBuf.readBytes(reencoded);
        } finally {
            outBuf.release();
        }

        JsonObject res = ok();
        res.addProperty("packet_type", packet.type().id().toString());
        res.addProperty("java_class", packet.getClass().getName());
        res.addProperty("bytes_b64", Base64.getEncoder().encodeToString(reencoded));
        return res;
    }

    private static JsonObject handleDumpSyncedRegistries(JsonObject req) throws Exception {
        boolean includeData = !req.has("include_data") || req.get("include_data").getAsBoolean();
        RegistryAccess.Frozen regs = getFullRegistries();
        RegistryOps<Tag> nbtOps = regs.createSerializationContext(NbtOps.INSTANCE);
        Set<KnownPack> knownPacks = includeData ? Set.of() : Set.of(KnownPack.vanilla("core"));

        JsonArray outRegs = new JsonArray();
        RegistrySynchronization.packRegistries(nbtOps, regs, knownPacks, (regKey, entries) -> {
            JsonObject regObj = new JsonObject();
            regObj.addProperty("registry", regKey.identifier().toString());
            JsonArray arr = new JsonArray();
            for (RegistrySynchronization.PackedRegistryEntry entry : entries) {
                JsonObject eo = new JsonObject();
                eo.addProperty("id", entry.id().toString());
                if (entry.data().isPresent()) {
                    try {
                        ByteArrayOutputStream baos = new ByteArrayOutputStream();
                        DataOutputStream dos = new DataOutputStream(baos);
                        NbtIo.writeAnyTag(entry.data().get(), dos);
                        dos.flush();
                        eo.addProperty("nbt_b64", Base64.getEncoder().encodeToString(baos.toByteArray()));
                    } catch (Exception e) {
                        throw new RuntimeException(e);
                    }
                }
                arr.add(eo);
            }
            regObj.add("entries", arr);
            outRegs.add(regObj);
        });

        JsonObject res = ok();
        res.add("registries", outRegs);
        return res;
    }

    private static JsonObject handleDumpNetworkTags() {
        LayeredRegistryAccess<RegistryLayer> layered = getLayeredRegistries();
        Map<ResourceKey<? extends Registry<?>>, TagNetworkSerialization.NetworkPayload> map =
            TagNetworkSerialization.serializeTagsToNetwork(layered);
        ByteBuf buf = Unpooled.buffer();
        try {
            net.minecraft.network.FriendlyByteBuf fbuf = new net.minecraft.network.FriendlyByteBuf(buf);
            net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket.STREAM_CODEC.encode(
                fbuf,
                new net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket(map)
            );
            byte[] bytes = new byte[buf.readableBytes()];
            buf.readBytes(bytes);
            JsonObject res = ok();
            res.addProperty("registry_count", map.size());
            res.addProperty("payload_b64", Base64.getEncoder().encodeToString(bytes));
            return res;
        } finally {
            buf.release();
        }
    }

    private static JsonObject handleDumpRuntimeTables(JsonObject req) throws Exception {
        getLayeredRegistries(); // Ensure vanilla BlockTags are bound
        Path outPath = Path.of(req.get("output").getAsString());
        JsonObject root = new JsonObject();

        // 1. Every BlockStateId's live physics & lighting properties
        JsonArray states = new JsonArray();
        int totalStates = Block.BLOCK_STATE_REGISTRY.size();
        for (int id = 0; id < totalStates; id++) {
            BlockState st = Block.BLOCK_STATE_REGISTRY.byId(id);
            if (st == null) continue;
            int flags = 0;
            if (st.isAir()) flags |= 1 << 0;
            if (st.propagatesSkylightDown()) flags |= 1 << 1;
            if (st.canOcclude()) flags |= 1 << 2;
            if (st.isSolid()) flags |= 1 << 3;
            if (st.canBeReplaced()) flags |= 1 << 4;
            if (!st.getFluidState().isEmpty()) flags |= 1 << 5;
            if (st.getFluidState().isSource()) flags |= 1 << 6;
            if (st.useShapeForLightOcclusion()) flags |= 1 << 7;
            if (st.requiresCorrectToolForDrops()) flags |= 1 << 8;
            if (st.is(net.minecraft.tags.BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP) || !st.getFluidState().isEmpty()) flags |= 1 << 9;
            if (st.is(net.minecraft.tags.BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES) || !st.getFluidState().isEmpty()) flags |= 1 << 10;

            Item asItem = st.getBlock().asItem();
            int itemId = BuiltInRegistries.ITEM.getId(asItem);

            JsonArray row = new JsonArray();
            row.add(st.getLightEmission());
            row.add(st.getLightDampening());
            row.add(flags);
            row.add(st.getDestroySpeed(net.minecraft.world.level.EmptyBlockGetter.INSTANCE, BlockPos.ZERO));
            row.add(itemId);
            states.add(row);
        }
        root.add("block_states", states);

        // 2. Every Item's id, name, max_stack_size, and BlockItem placement state
        JsonArray items = new JsonArray();
        int totalItems = BuiltInRegistries.ITEM.size();
        for (int id = 0; id < totalItems; id++) {
            Item item = BuiltInRegistries.ITEM.byId(id);
            Identifier key = BuiltInRegistries.ITEM.getKey(item);
            int placesStateId = -1;
            if (item instanceof BlockItem bi) {
                placesStateId = Block.BLOCK_STATE_REGISTRY.getId(bi.getBlock().defaultBlockState());
            }
            JsonArray row = new JsonArray();
            row.add(id);
            row.add(key.toString());
            row.add(item.getDefaultMaxStackSize());
            row.add(placesStateId);
            items.add(row);
        }
        root.add("items", items);

        java.nio.file.Files.createDirectories(outPath.getParent());
        java.nio.file.Files.writeString(outPath, GSON.toJson(root), StandardCharsets.UTF_8);

        JsonObject res = ok();
        res.addProperty("block_state_count", states.size());
        res.addProperty("item_count", items.size());
        res.addProperty("output", outPath.toString());
        return res;
    }

    private static JsonObject handleNoiseSample(JsonObject req) {
        String kind = req.get("kind").getAsString();
        long seed = req.get("seed").getAsLong();
        JsonArray coords = req.getAsJsonArray("coords");
        JsonArray outBits = new JsonArray();

        if ("perlin".equals(kind)) {
            PerlinNoise noise = new PerlinNoise(new XoroshiroRandomSource(seed));
            for (JsonElement el : coords) {
                JsonArray c = el.getAsJsonArray();
                float v = noise.get(c.get(0).getAsDouble(), c.get(1).getAsDouble(), c.get(2).getAsDouble());
                outBits.add(Float.floatToRawIntBits(v));
            }
        } else if ("normal".equals(kind)) {
            String noiseId = req.get("noise_id").getAsString();
            Registry<NormalNoise> noiseReg = getFullRegistries().lookupOrThrow(Registries.NOISE);
            NormalNoise def = noiseReg.getValueOrThrow(ResourceKey.create(Registries.NOISE, Identifier.parse(noiseId)));
            Noise inst = def.create(new XoroshiroRandomSource(seed));
            for (JsonElement el : coords) {
                JsonArray c = el.getAsJsonArray();
                float v = inst.get(c.get(0).getAsDouble(), c.get(1).getAsDouble(), c.get(2).getAsDouble());
                outBits.add(Float.floatToRawIntBits(v));
            }
        } else {
            throw new IllegalArgumentException("Unknown noise kind: " + kind);
        }

        JsonObject res = ok();
        res.add("values_bits", outBits);
        return res;
    }

    private static JsonObject handleClimateSample(JsonObject req) {
        RegistryAccess.Frozen regs = getFullRegistries();
        Registry<MultiNoiseBiomeSourceParameterList> paramReg =
            regs.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST);
        MultiNoiseBiomeSourceParameterList overworld =
            paramReg.getValueOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD);
        Climate.ParameterList<net.minecraft.core.Holder<Biome>> params = overworld.parameters();

        JsonArray targets = req.getAsJsonArray("targets");
        JsonArray biomes = new JsonArray();
        for (JsonElement el : targets) {
            JsonArray t = el.getAsJsonArray();
            Climate.TargetPoint tp = new Climate.TargetPoint(
                t.get(0).getAsLong(),
                t.get(1).getAsLong(),
                t.get(2).getAsLong(),
                t.get(3).getAsLong(),
                t.get(4).getAsLong(),
                t.get(5).getAsLong()
            );
            String biomeId = params.findValue(tp).unwrapKey().orElseThrow().identifier().toString();
            biomes.add(biomeId);
        }
        JsonObject res = ok();
        res.add("biomes", biomes);
        return res;
    }

    private static JsonObject handleNoiseRouterSample(JsonObject req) {
        long seed = req.get("seed").getAsLong();
        RegistryAccess.Frozen regs = getFullRegistries();
        Registry<NormalNoise> noiseReg = regs.lookupOrThrow(Registries.NOISE);
        Registry<net.minecraft.world.level.levelgen.NoiseGeneratorSettings> settingsReg =
            regs.lookupOrThrow(Registries.NOISE_SETTINGS);
        net.minecraft.world.level.levelgen.NoiseGeneratorSettings overworldSettings =
            settingsReg.getValueOrThrow(net.minecraft.world.level.levelgen.NoiseGeneratorSettings.OVERWORLD);
        net.minecraft.world.level.levelgen.RandomState rs =
            net.minecraft.world.level.levelgen.RandomState.create(noiseReg, seed, overworldSettings);
        net.minecraft.world.level.levelgen.NoiseRouter router = overworldSettings.noiseRouter();

        Registry<MultiNoiseBiomeSourceParameterList> paramReg =
            regs.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST);
        Climate.ParameterList<net.minecraft.core.Holder<Biome>> biomeParams =
            paramReg.getValueOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD).parameters();

        JsonArray coords = req.getAsJsonArray("coords");
        JsonArray samples = new JsonArray();
        for (JsonElement el : coords) {
            JsonArray c = el.getAsJsonArray();
            int x = c.get(0).getAsInt();
            int y = c.get(1).getAsInt();
            int z = c.get(2).getAsInt();

            float t = rs.sampleBlockValueUncached(router.temperature(), x, y, z);
            float v = rs.sampleBlockValueUncached(router.vegetation(), x, y, z);
            float ct = rs.sampleBlockValueUncached(router.continents(), x, y, z);
            float er = rs.sampleBlockValueUncached(router.erosion(), x, y, z);
            float d = rs.sampleBlockValueUncached(router.depth(), x, y, z);
            float r = rs.sampleBlockValueUncached(router.ridges(), x, y, z);
            float surf = rs.sampleBlockValueUncached(router.chunkSurfaceLevel(), x, y, z);
            float fd = rs.sampleBlockValueUncached(router.finalDensity(), x, y, z);

            Climate.TargetPoint tp = Climate.target(t, v, ct, er, d, r);
            String biome = biomeParams.findValue(tp).unwrapKey().orElseThrow().identifier().toString();

            JsonObject row = new JsonObject();
            row.addProperty("temperature_bits", Float.floatToRawIntBits(t));
            row.addProperty("vegetation_bits", Float.floatToRawIntBits(v));
            row.addProperty("continents_bits", Float.floatToRawIntBits(ct));
            row.addProperty("erosion_bits", Float.floatToRawIntBits(er));
            row.addProperty("depth_bits", Float.floatToRawIntBits(d));
            row.addProperty("ridges_bits", Float.floatToRawIntBits(r));
            row.addProperty("chunk_surface_level_bits", Float.floatToRawIntBits(surf));
            row.addProperty("final_density_bits", Float.floatToRawIntBits(fd));
            row.addProperty("biome", biome);
            samples.add(row);
        }

        JsonObject res = ok();
        res.add("samples", samples);
        return res;
    }
}
