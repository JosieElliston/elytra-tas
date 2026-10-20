package jealoustone.elytratas;

import dev.isxander.yacl3.api.Binding;
import dev.isxander.yacl3.api.ConfigCategory;
import dev.isxander.yacl3.api.Option;
import dev.isxander.yacl3.api.OptionDescription;
import dev.isxander.yacl3.api.YetAnotherConfigLib;
import dev.isxander.yacl3.api.controller.TickBoxControllerBuilder;
import jealoustone.elytratas.netlog.NetLog;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.network.chat.Component;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;
import java.util.function.Consumer;
import java.util.function.Supplier;

/** Fields are volatile because Netty and integrated-server threads read them. */
public final class LoggerConfig {
    public volatile boolean enabled = true;
    public volatile boolean autoStart = true;
    public volatile boolean logTicks = true;
    public volatile boolean logOutbound = true;
    public volatile boolean logInbound = true;
    public volatile boolean logOtherInbound = true;
    public volatile boolean logWire = true;
    public volatile boolean logServer = true;

    private final Path path = FabricLoader.getInstance().getConfigDir().resolve("elytra-tas.properties");

    public void load() {
        if (!Files.exists(path)) return;
        Properties properties = new Properties();
        try (InputStream in = Files.newInputStream(path)) {
            properties.load(in);
            enabled = flag(properties, "enabled");
            autoStart = flag(properties, "autoStart");
            logTicks = flag(properties, "logTicks");
            logOutbound = flag(properties, "logOutbound");
            logInbound = flag(properties, "logInbound");
            logOtherInbound = flag(properties, "logOtherInbound");
            logWire = flag(properties, "logWire");
            logServer = flag(properties, "logServer");
        } catch (IOException e) {
            NetLog.LOGGER.error("elytra-tas: could not read settings", e);
        }
    }

    private void save() {
        Properties properties = new Properties();
        properties.setProperty("enabled", Boolean.toString(enabled));
        properties.setProperty("autoStart", Boolean.toString(autoStart));
        properties.setProperty("logTicks", Boolean.toString(logTicks));
        properties.setProperty("logOutbound", Boolean.toString(logOutbound));
        properties.setProperty("logInbound", Boolean.toString(logInbound));
        properties.setProperty("logOtherInbound", Boolean.toString(logOtherInbound));
        properties.setProperty("logWire", Boolean.toString(logWire));
        properties.setProperty("logServer", Boolean.toString(logServer));
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
                .save(this::save)
                .build().generateScreen(parent);
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
