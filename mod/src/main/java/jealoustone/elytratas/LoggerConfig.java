package jealoustone.elytratas;

import dev.isxander.yacl3.api.Binding;
import dev.isxander.yacl3.api.ButtonOption;
import dev.isxander.yacl3.api.ConfigCategory;
import dev.isxander.yacl3.api.LabelOption;
import dev.isxander.yacl3.api.ListOption;
import dev.isxander.yacl3.api.Option;
import dev.isxander.yacl3.api.OptionDescription;
import dev.isxander.yacl3.api.YetAnotherConfigLib;
import dev.isxander.yacl3.api.controller.StringControllerBuilder;
import dev.isxander.yacl3.api.controller.TickBoxControllerBuilder;
import jealoustone.elytratas.netlog.NetLog;
import jealoustone.elytratas.phase.ClientPhase;
import net.minecraft.client.Minecraft;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.multiplayer.resolver.ServerAddress;
import net.minecraft.network.chat.Component;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collection;
import java.util.List;
import java.util.Locale;
import java.util.Properties;
import java.util.function.Consumer;
import java.util.function.Supplier;

/** Fields are volatile because Netty and integrated-server threads read them. */
public final class LoggerConfig {
    public volatile boolean enabled = true;
    public volatile boolean autoStart = true;
    public volatile boolean logTicks = true;
    public volatile boolean logKeys = true;
    public volatile boolean logOutbound = true;
    public volatile boolean logInbound = true;
    public volatile boolean logOtherInbound = true;
    public volatile boolean logWire = true;
    public volatile boolean logServer = true;
    /**
     * user@host pairs where the mod may send packets of its own (the pings and probes of
     * /etas phase); hosts lowercase, without ports. why? everything else only observes; extra
     * packets are for accounts and servers that agreed to them. An entry without an @ never
     * matches.
     */
    public volatile List<String> whitelist = DEFAULT_WHITELIST;

    private static final List<String> DEFAULT_WHITELIST = List.of("deftenvy@elytramc.net");

    private final Path path = FabricLoader.getInstance().getConfigDir().resolve("elytra-tas.properties");

    public void load() {
        if (!Files.exists(path)) return;
        Properties properties = new Properties();
        try (InputStream in = Files.newInputStream(path)) {
            properties.load(in);
            enabled = flag(properties, "enabled");
            autoStart = flag(properties, "autoStart");
            logTicks = flag(properties, "logTicks");
            logKeys = flag(properties, "logKeys");
            logOutbound = flag(properties, "logOutbound");
            logInbound = flag(properties, "logInbound");
            logOtherInbound = flag(properties, "logOtherInbound");
            logWire = flag(properties, "logWire");
            logServer = flag(properties, "logServer");
            whitelist = entries(List.of(properties.getProperty("whitelist",
                    String.join(",", DEFAULT_WHITELIST)).split(",")));
        } catch (IOException e) {
            NetLog.LOGGER.error("elytra-tas: could not read settings", e);
        }
    }

    private void save() {
        Properties properties = new Properties();
        properties.setProperty("enabled", Boolean.toString(enabled));
        properties.setProperty("autoStart", Boolean.toString(autoStart));
        properties.setProperty("logTicks", Boolean.toString(logTicks));
        properties.setProperty("logKeys", Boolean.toString(logKeys));
        properties.setProperty("logOutbound", Boolean.toString(logOutbound));
        properties.setProperty("logInbound", Boolean.toString(logInbound));
        properties.setProperty("logOtherInbound", Boolean.toString(logOtherInbound));
        properties.setProperty("logWire", Boolean.toString(logWire));
        properties.setProperty("logServer", Boolean.toString(logServer));
        properties.setProperty("whitelist", String.join(",", whitelist));
        try (OutputStream out = Files.newOutputStream(path)) {
            properties.store(out, "Elytra TAS settings");
        } catch (IOException e) {
            NetLog.LOGGER.error("elytra-tas: could not save settings", e);
        }
        if (!enabled) NetLog.stop("disabled");
    }

    public Screen screen(Screen parent) {
        return YetAnotherConfigLib.createBuilder()
                .title(Component.literal("Elytra TAS"))
                .category(ConfigCategory.createBuilder()
                        .name(Component.literal("Network log"))
                        .option(toggle("Logger enabled", "Off stops any running log and ignores the toggle key.",
                                () -> enabled, value -> enabled = value))
                        .option(toggle("Start on join", "Start a new log when joining a server or world. Otherwise use the toggle key (unbound by default).",
                                () -> autoStart, value -> autoStart = value))
                        .option(toggle("Client ticks", "Local player pose, velocity, flags, and keys at the start and end of every client tick.",
                                () -> logTicks, value -> logTicks = value))
                        .option(toggle("Key presses", "Every key mapping going down or up (jump, use, hotbar, and so on), timed when the client handled the key event, between ticks too.",
                                () -> logKeys, value -> logKeys = value))
                        .option(toggle("Outbound packets", "Every packet sent, with movement, command, input, keep-alive, and pong contents.",
                                () -> logOutbound, value -> logOutbound = value))
                        .option(toggle("Inbound packets", "Decode and handle times of received packets; teleports, own motion and entity data, keep-alives, pings, chat, titles, and dimension changes in full.",
                                () -> logInbound, value -> logInbound = value))
                        .option(toggle("Other inbound packets", "Also log inbound packets without decoded contents, by name only. The bulk of the log.",
                                () -> logOtherInbound, value -> logOtherInbound = value))
                        .option(toggle("Socket reads and writes", "Times when frames reach the socket and raw reads come off it.",
                                () -> logWire, value -> logWire = value))
                        .option(toggle("Integrated server", "In singleplayer, also log the server's view: move packets processed, player ticks, and glide ends.",
                                () -> logServer, value -> logServer = value))
                        .build())
                .category(ConfigCategory.createBuilder()
                        .name(Component.literal("Whitelist"))
                        .group(ListOption.<String>createBuilder()
                                .name(Component.literal("Whitelist"))
                                .description(OptionDescription.of(Component.literal("Accounts and servers where the mod "
                                        + "may send packets of its own, as user@server: the pings and tab-completion "
                                        + "probes behind /etas phase. Elsewhere it only observes. A server counts with "
                                        + "its subdomains, and ports are ignored. Singleplayer always counts.")))
                                .binding(DEFAULT_WHITELIST, () -> whitelist, value -> whitelist = entries(value))
                                .controller(StringControllerBuilder::create)
                                .initial("")
                                .build())
                        .build())
                .category(phaseCategory())
                .save(this::save)
                .build().generateScreen(parent);
    }

    /**
     * Whether the mod may send its own packets as this user to a server, by its address as
     * typed; a null address is singleplayer.
     */
    public boolean whitelisted(String user, String address) {
        if (address == null) return true;
        String host = host(address);
        for (String entry : whitelist) {
            int at = entry.indexOf('@');
            if (at < 0 || !entry.substring(0, at).equalsIgnoreCase(user)) continue;
            String w = entry.substring(at + 1);
            if (host.equals(w) || host.endsWith("." + w)) return true;
        }
        return false;
    }

    private static String host(String address) {
        return ServerAddress.parseString(address.strip()).getHost().toLowerCase(Locale.ROOT);
    }

    /** Normalizes user@server entries: trims, lowercases the host and drops its port. Others stay as typed. */
    private static List<String> entries(Collection<String> entries) {
        return entries.stream().map(String::strip).filter(e -> !e.isEmpty()).map(e -> {
            int at = e.indexOf('@');
            return at < 0 ? e : e.substring(0, at).strip() + "@" + host(e.substring(at + 1));
        }).distinct().toList();
    }

    /** Not a setting: buttons that act at once, and only while not in a world (see ClientPhase). */
    private static ConfigCategory phaseCategory() {
        Minecraft client = Minecraft.getInstance();
        boolean can = ClientPhase.canShift(client);
        double phase = ClientPhase.phase(client);
        String state = String.format("Your client ticks at %s ms past each 50 ms of the system clock, delayed %d ms "
                + "since launch.", Double.isNaN(phase) ? "?" : String.format("%.1f", phase), ClientPhase.delayed());
        ConfigCategory.Builder b = ConfigCategory.createBuilder()
                .name(Component.literal("Tick phase"))
                .option(LabelOption.create(Component.literal(state + (can ? " Each button moves it at once; a "
                        + "toast shows the total." : " Leave the world to change it."))));
        for (int ms : new int[]{1, 10, -1, -10}) {
            int delay = Math.floorMod(ms, 50);
            b.option(ButtonOption.createBuilder()
                    .name(Component.literal(String.format("%+d ms", ms)))
                    .text(Component.literal("Delay " + delay + " ms"))
                    .description(OptionDescription.of(Component.literal("Moves your client's ticks " + delay
                            + " ms later. A shift is always a delay, so ticks never come closer than 50 ms. Only "
                            + "while not in a world: a server could see your tick phase move within a session.")))
                    .available(can)
                    .action((screen, button) -> ClientPhase.shift(Minecraft.getInstance(), ms))
                    .build());
        }
        return b.build();
    }

    private static boolean flag(Properties properties, String key) {
        return Boolean.parseBoolean(properties.getProperty(key, "true"));
    }

    private static Option<Boolean> toggle(String label, String description, Supplier<Boolean> get, Consumer<Boolean> set) {
        return Option.<Boolean>createBuilder()
                .name(Component.literal(label))
                .description(OptionDescription.of(Component.literal(description)))
                .binding(Binding.generic(true, get, set))
                .controller(TickBoxControllerBuilder::create)
                .build();
    }
}
