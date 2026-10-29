package jealoustone.elytratas;

import com.mojang.blaze3d.platform.InputConstants;
import jealoustone.elytratas.netlog.NetLog;
import jealoustone.elytratas.phase.PhaseCommand;
import jealoustone.elytratas.phase.PhaseTracker;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.command.v2.ClientCommandRegistrationCallback;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientLifecycleEvents;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.fabricmc.fabric.api.client.keymapping.v1.KeyMappingHelper;
import net.fabricmc.fabric.api.client.networking.v1.ClientPlayConnectionEvents;
import net.minecraft.client.KeyMapping;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;

public final class ElytraTasClient implements ClientModInitializer {
    public static final LoggerConfig CONFIG = new LoggerConfig();
    private static KeyMapping toggleKey;

    @Override
    public void onInitializeClient() {
        CONFIG.load();
        KeyMapping.Category category = KeyMapping.Category.register(Identifier.fromNamespaceAndPath("elytra-tas", "controls"));
        toggleKey = KeyMappingHelper.registerKeyMapping(new KeyMapping(
                "key.elytra-tas.toggle_log", InputConstants.Type.KEYSYM, InputConstants.UNKNOWN.getValue(), category));

        // INIT runs again on the same Connection after a server-driven reconfiguration;
        // that continues the session instead of starting a new file.
        ClientPlayConnectionEvents.INIT.register((handler, client) -> {
            PhaseTracker.init(handler.getConnection());
            if (CONFIG.enabled && CONFIG.autoStart && NetLog.connection() != handler.getConnection()) {
                NetLog.start(handler.getConnection(), handler.getServerData(), client);
            }
        });
        ClientPlayConnectionEvents.DISCONNECT.register((handler, client) -> PhaseTracker.disconnect());
        ClientLifecycleEvents.CLIENT_STOPPING.register(client -> NetLog.stop("exit"));
        ClientCommandRegistrationCallback.EVENT.register((dispatcher, context) -> PhaseCommand.register(dispatcher));

        ClientTickEvents.START_CLIENT_TICK.register(client -> {
            PhaseTracker.tickStart(client);
            NetLog.clientTick(client, true);
        });
        ClientTickEvents.END_CLIENT_TICK.register(client -> {
            NetLog.clientTick(client, false);
            PhaseTracker.tickEnd(client);
            while (toggleKey.consumeClick()) {
                if (client.player == null || client.getConnection() == null) continue;
                if (NetLog.active()) {
                    NetLog.stop("key");
                    client.player.sendOverlayMessage(Component.literal("elytra-tas: log stopped"));
                } else if (CONFIG.enabled) {
                    NetLog.start(client.getConnection().getConnection(), client.getCurrentServer(), client);
                    if (NetLog.active()) {
                        client.player.sendOverlayMessage(Component.literal("elytra-tas: logging to " + NetLog.path().getFileName()));
                    }
                }
            }
        });
    }
}
