package jealoustone.elytratas.netlog;

import io.netty.channel.Channel;
import jealoustone.elytratas.ElytraTasClient;
import jealoustone.elytratas.LoggerConfig;
import jealoustone.elytratas.mixin.ConnectionAccessor;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.SharedConstants;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.Connection;
import net.minecraft.network.protocol.BundleDelimiterPacket;
import net.minecraft.network.protocol.BundlePacket;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.common.ClientboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.common.ServerboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ServerboundPongPacket;
import net.minecraft.network.protocol.game.ClientboundLoginPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.ClientboundRespawnPacket;
import net.minecraft.network.protocol.game.ClientboundSetActionBarTextPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket;
import net.minecraft.network.protocol.game.ClientboundSetSubtitleTextPacket;
import net.minecraft.network.protocol.game.ClientboundSetTitleTextPacket;
import net.minecraft.network.protocol.game.ClientboundSystemChatPacket;
import net.minecraft.network.protocol.game.CommonPlayerSpawnInfo;
import net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerCommandPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerInputPacket;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.entity.PositionMoveRotation;
import net.minecraft.world.entity.Relative;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.phys.Vec3;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.Collections;
import java.util.IdentityHashMap;
import java.util.Map;
import java.util.UUID;

/**
 * Network/tick logger. One session per server connection, one gzipped TSV per session.
 * The record format is documented in netlog.py (repo root), which parses it; change
 * both together and bump {@link #FORMAT}.
 *
 * Threads: tick and key records and H(main) come from the client thread; W/F/R/D and H(netty)
 * from the connection's Netty event loop (one thread per channel); S from either; X
 * from the integrated server thread.
 */
public final class NetLog {
    public static final Logger LOGGER = LoggerFactory.getLogger("elytra-tas");
    public static final String FORMAT = "elytra-tas-netlog\t2";
    private static final DateTimeFormatter FILE_NAME = DateTimeFormatter.ofPattern("yyyy-MM-dd_HH-mm-ss");
    private static final int PENDING_LIMIT = 100_000;

    /** Number of the most recent START_CLIENT_TICK in this session (0 before the first). */
    public static volatile int tick;

    private static volatile LogWriter writer;
    private static volatile Connection connection;
    private static volatile Channel channel;
    private static volatile UUID localUuid;
    private static volatile int selfId = Integer.MIN_VALUE;

    private static long outSeq;
    private static long inSeq;
    private static long readSeq;
    // Netty-thread-only state for the session channel (a channel never changes event loop).
    private static long pendingWireSeq = -1;
    private static long inlineStart;

    private static final Map<Packet<?>, Long> outPending = Collections.synchronizedMap(new IdentityHashMap<>());
    private static final Map<Packet<?>, Long> inPending = Collections.synchronizedMap(new IdentityHashMap<>());

    private NetLog() {
    }

    private static LoggerConfig config() {
        return ElytraTasClient.CONFIG;
    }

    public static boolean active() {
        return writer != null;
    }

    public static Connection connection() {
        return connection;
    }

    public static Path path() {
        LogWriter w = writer;
        return w == null ? null : w.path;
    }

    public static synchronized void start(Connection conn, ServerData server, Minecraft client) {
        if (writer != null) stop("restart");
        Path dir = client.gameDirectory.toPath().resolve("elytra-tas").resolve("logs");
        String stem = LocalDateTime.now().format(FILE_NAME);
        Path file = dir.resolve(stem + ".tsv.gz");
        for (int i = 2; Files.exists(file); i++) file = dir.resolve(stem + "-" + i + ".tsv.gz");
        LogWriter w;
        try {
            w = new LogWriter(file);
        } catch (IOException e) {
            LOGGER.error("elytra-tas: could not open {}", file, e);
            return;
        }
        tick = 0;
        outSeq = inSeq = readSeq = 0;
        pendingWireSeq = -1;
        outPending.clear();
        inPending.clear();
        localUuid = client.getUser().getProfileId();
        selfId = client.player == null ? Integer.MIN_VALUE : client.player.getId();
        connection = conn;
        channel = ((ConnectionAccessor) conn).elytraTas$channel();
        w.offer("#" + FORMAT);
        Line meta = new Line('M').l(System.currentTimeMillis())
                .s("mod=" + FabricLoader.getInstance().getModContainer("elytra-tas")
                        .map(m -> m.getMetadata().getVersion().getFriendlyString()).orElse("?"))
                .s("mc=" + SharedConstants.getCurrentVersion().name())
                .s("protocol=" + SharedConstants.getCurrentVersion().protocolVersion())
                .s("player=" + client.getUser().getName())
                .s("uuid=" + localUuid)
                .s("singleplayer=" + (client.hasSingleplayerServer() ? 1 : 0))
                .s("server=" + (server == null ? "-" : server.ip))
                .s("memory=" + (conn.isMemoryConnection() ? 1 : 0));
        w.offer(meta.toString());
        writer = w;
        LOGGER.info("elytra-tas: logging to {}", file);
    }

    public static synchronized void stop(String reason) {
        LogWriter w = writer;
        if (w == null) return;
        w.offer(new Line('E').s("stop").s(reason).toString());
        writer = null;
        connection = null;
        channel = null;
        outPending.clear();
        inPending.clear();
        w.close();
        LOGGER.info("elytra-tas: closed {}", w.path);
    }

    // --- client ticks ---------------------------------------------------------------

    public static void clientTick(Minecraft client, boolean start) {
        if (writer == null) return;
        if (start) tick++;
        LocalPlayer player = client.player;
        if (player == null) return;
        selfId = player.getId();
        if (!config().logTicks) return;
        Vec3 pos = player.position();
        Vec3 v = player.getDeltaMovement();
        new Line('T').c(start ? 's' : 'e')
                .d(pos.x).d(pos.y).d(pos.z).f(player.getYRot()).f(player.getXRot())
                .d(v.x).d(v.y).d(v.z)
                .b(player.onGround()).b(player.horizontalCollision).b(player.isFallFlying()).b(player.isSprinting())
                .l(inputBits(player.input.keyPresses))
                .emit();
    }

    /** KeyMapping.setDown changing the mapping's state, client thread. */
    public static void key(String name, boolean down) {
        if (writer == null || !config().logKeys) return;
        new Line('K').s(name).b(down).emit();
    }

    // --- outbound -------------------------------------------------------------------

    /** Connection.send, on whatever thread called it. */
    public static void sent(Connection conn, Packet<?> packet) {
        if (writer == null || conn != connection || !config().logOutbound) return;
        Line line = new Line('S');
        long seq;
        synchronized (outPending) {
            seq = ++outSeq;
            if (outPending.size() > PENDING_LIMIT) outPending.clear();
            outPending.put(packet, seq);
        }
        line.c(Minecraft.getInstance().isSameThread() ? 'm' : 'n').l(seq).s(name(packet));
        describeOutbound(line, packet);
        line.emit();
    }

    /** PacketEncoder.encode returned, Netty thread. The encoded frame goes to the wire tap next. */
    public static void encoded(Channel ch, Packet<?> packet) {
        if (ch != channel) return;
        Long seq = outPending.remove(packet);
        pendingWireSeq = seq == null ? -1 : seq;
    }

    /** Wire tap write: the frame reached the head of the pipeline, Netty thread. */
    public static void wireWrite(Channel ch, int bytes) {
        if (writer == null || ch != channel) return;
        long seq = pendingWireSeq;
        pendingWireSeq = -1;
        if (!config().logWire) return;
        new Line('W').l(seq).l(bytes).emit();
    }

    /** Wire tap flush: started at {@code startNanos}, socket write returned now. */
    public static void wireFlush(Channel ch, long startNanos) {
        if (writer == null || ch != channel || !config().logWire) return;
        long end = System.nanoTime();
        new Line('F', startNanos).l(end).emit();
    }

    // --- inbound --------------------------------------------------------------------

    /** Wire tap read: bytes came off the socket, Netty thread. */
    public static void wireRead(Channel ch, int bytes) {
        if (writer == null || ch != channel) return;
        readSeq++;
        if (!config().logWire) return;
        new Line('R').l(readSeq).l(bytes).emit();
    }

    /** PacketDecoder.decode produced {@code packet}, Netty thread. */
    public static void decoded(Channel ch, Packet<?> packet) {
        if (writer == null || ch != channel || !config().logInbound || !wantedInbound(packet)) return;
        Line line = new Line('D');
        long seq;
        synchronized (inPending) {
            seq = ++inSeq;
            if (!(packet instanceof BundleDelimiterPacket)) {
                if (inPending.size() > PENDING_LIMIT) inPending.clear();
                inPending.put(packet, seq);
            }
        }
        line.l(seq).l(readSeq).s(name(packet)).emit();
    }

    /** Connection.genericsFtw entry, Netty thread: the packet's handler is about to run there. */
    public static void inlineHandleStart(Packet<?> packet) {
        if (writer != null && inPending.containsKey(packet)) inlineStart = System.nanoTime();
    }

    /** Connection.genericsFtw returned normally: the handler ran on the Netty thread, not queued. */
    public static void inlineHandled(Packet<?> packet) {
        if (writer == null) return;
        Long seq = inPending.remove(packet);
        if (seq != null) handled(new Line('H', inlineStart).c('n'), seq, packet);
    }

    /** PacketProcessor queue entry about to run on the client thread. */
    public static void mainHandle(Packet<?> packet) {
        if (writer == null) return;
        long now = System.nanoTime();
        if (packet instanceof BundlePacket<?> bundle) {
            if (config().logInbound && config().logOtherInbound) handled(new Line('H', now).c('m'), -1, packet);
            for (Packet<?> sub : bundle.subPackets()) mainHandle(sub);
            return;
        }
        Long seq = inPending.remove(packet);
        if (seq != null) handled(new Line('H', now).c('m'), seq, packet);
    }

    private static void handled(Line line, long seq, Packet<?> packet) {
        line.l(seq).s(name(packet));
        describeInbound(line, packet);
        line.emit();
    }

    private static boolean wantedInbound(Packet<?> packet) {
        if (config().logOtherInbound) return true;
        return switch (packet) {
            case ClientboundSetEntityMotionPacket p -> p.id() == selfId;
            case ClientboundSetEntityDataPacket p -> p.id() == selfId;
            default -> isDescribedInbound(packet);
        };
    }

    private static boolean isDescribedInbound(Packet<?> packet) {
        return packet instanceof ClientboundPlayerPositionPacket || packet instanceof ClientboundKeepAlivePacket
                || packet instanceof ClientboundPingPacket || packet instanceof ClientboundSystemChatPacket
                || packet instanceof ClientboundSetActionBarTextPacket || packet instanceof ClientboundSetTitleTextPacket
                || packet instanceof ClientboundSetSubtitleTextPacket || packet instanceof ClientboundRespawnPacket
                || packet instanceof ClientboundLoginPacket;
    }

    // --- integrated server ------------------------------------------------------------

    public static boolean serverWanted(ServerPlayer player) {
        return writer != null && config().logServer && player.getUUID().equals(localUuid);
    }

    /** Server-side record; caller checked {@link #serverWanted}. */
    public static void server(String event, ServerPlayer player, String... extra) {
        Vec3 pos = player.position();
        Vec3 v = player.getDeltaMovement();
        Line line = new Line('X').s(event).l(player.level().getServer().getTickCount())
                .d(pos.x).d(pos.y).d(pos.z).d(v.x).d(v.y).d(v.z)
                .b(player.onGround()).b(player.isFallFlying());
        for (String s : extra) line.s(s);
        line.emit();
    }

    // --- packet details ---------------------------------------------------------------

    public static String name(Packet<?> packet) {
        Identifier id = packet.type().id();
        return Identifier.DEFAULT_NAMESPACE.equals(id.getNamespace()) ? id.getPath() : id.toString();
    }

    private static void describeOutbound(Line line, Packet<?> packet) {
        switch (packet) {
            case ServerboundMovePlayerPacket p -> {
                if (p.hasPosition()) line.d(p.getX(0)).d(p.getY(0)).d(p.getZ(0));
                else line.s("-").s("-").s("-");
                if (p.hasRotation()) line.f(p.getYRot(0)).f(p.getXRot(0));
                else line.s("-").s("-");
                line.b(p.isOnGround()).b(p.horizontalCollision());
            }
            case ServerboundPlayerCommandPacket p -> line.s(p.getAction().name()).l(p.getData());
            case ServerboundPlayerInputPacket p -> line.l(inputBits(p.input()));
            case ServerboundKeepAlivePacket p -> line.l(p.getId());
            case ServerboundPongPacket p -> line.l(p.getId());
            case ServerboundAcceptTeleportationPacket p -> line.l(p.getId());
            default -> {
            }
        }
    }

    private static void describeInbound(Line line, Packet<?> packet) {
        switch (packet) {
            case ClientboundPlayerPositionPacket p -> {
                PositionMoveRotation c = p.change();
                StringBuilder rel = new StringBuilder();
                for (Relative r : Relative.values()) {
                    if (!p.relatives().contains(r)) continue;
                    if (!rel.isEmpty()) rel.append(',');
                    rel.append(r.name());
                }
                line.l(p.id()).d(c.position().x).d(c.position().y).d(c.position().z)
                        .d(c.deltaMovement().x).d(c.deltaMovement().y).d(c.deltaMovement().z)
                        .f(c.yRot()).f(c.xRot()).s(rel.isEmpty() ? "-" : rel.toString());
            }
            case ClientboundSetEntityMotionPacket p -> {
                if (p.id() == selfId) line.s("self").d(p.movement().x).d(p.movement().y).d(p.movement().z);
            }
            case ClientboundSetEntityDataPacket p -> {
                if (p.id() != selfId) return;
                StringBuilder items = new StringBuilder();
                for (SynchedEntityData.DataValue<?> item : p.packedItems()) {
                    if (!items.isEmpty()) items.append(';');
                    items.append(item.id()).append('=');
                    if (item.value() instanceof Byte b) items.append("0x").append(Integer.toHexString(b & 0xFF));
                    else items.append(truncate(String.valueOf(item.value()), 120).replace(';', ','));
                }
                line.s("self").s(items.toString());
            }
            case ClientboundKeepAlivePacket p -> line.l(p.getId());
            case ClientboundPingPacket p -> line.l(p.getId());
            case ClientboundSystemChatPacket p -> line.b(p.overlay()).s(truncate(p.content().getString(), 1000));
            case ClientboundSetActionBarTextPacket p -> line.s(truncate(p.text().getString(), 1000));
            case ClientboundSetTitleTextPacket p -> line.s(truncate(p.text().getString(), 1000));
            case ClientboundSetSubtitleTextPacket p -> line.s(truncate(p.text().getString(), 1000));
            case ClientboundRespawnPacket p -> spawnInfo(line, p.commonPlayerSpawnInfo()).l(p.dataToKeep());
            case ClientboundLoginPacket p -> spawnInfo(line, p.commonPlayerSpawnInfo()).l(p.playerId());
            default -> {
            }
        }
    }

    private static Line spawnInfo(Line line, CommonPlayerSpawnInfo info) {
        return line.s(info.dimension().identifier().toString()).s(info.gameType().getName());
    }

    private static String truncate(String s, int max) {
        return s.length() <= max ? s : s.substring(0, max) + "...";
    }

    static int inputBits(Input input) {
        return (input.forward() ? 1 : 0) | (input.backward() ? 2 : 0) | (input.left() ? 4 : 0) | (input.right() ? 8 : 0)
                | (input.jump() ? 16 : 0) | (input.shift() ? 32 : 0) | (input.sprint() ? 64 : 0);
    }

    /** One TSV record: type, nanoTime, tick, then fields. Doubles and floats are written exactly. */
    static final class Line {
        private final StringBuilder sb = new StringBuilder(160);

        Line(char type) {
            this(type, System.nanoTime());
        }

        Line(char type, long nanos) {
            sb.append(type).append('\t').append(nanos).append('\t').append(tick);
        }

        Line s(String value) {
            sb.append('\t');
            for (int i = 0; i < value.length(); i++) {
                char ch = value.charAt(i);
                switch (ch) {
                    case '\\' -> sb.append("\\\\");
                    case '\t' -> sb.append("\\t");
                    case '\n' -> sb.append("\\n");
                    case '\r' -> sb.append("\\r");
                    default -> sb.append(ch);
                }
            }
            return this;
        }

        Line c(char value) {
            sb.append('\t').append(value);
            return this;
        }

        Line l(long value) {
            sb.append('\t').append(value);
            return this;
        }

        Line b(boolean value) {
            sb.append('\t').append(value ? '1' : '0');
            return this;
        }

        /** Shortest decimal that parses back to the same double (Java 19+ Double.toString). */
        Line d(double value) {
            sb.append('\t').append(value);
            return this;
        }

        /**
         * A float, widened to double first. why? the decimal then parses to exactly the
         * float's value in any double-based reader; Float.toString's shorter form would
         * need a float parse to round-trip.
         */
        Line f(float value) {
            sb.append('\t').append((double) value);
            return this;
        }

        void emit() {
            LogWriter w = writer;
            if (w != null) w.offer(sb.toString());
        }

        @Override
        public String toString() {
            return sb.toString();
        }
    }
}
