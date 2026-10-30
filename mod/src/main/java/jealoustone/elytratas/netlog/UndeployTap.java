package jealoustone.elytratas.netlog;

import java.util.function.BiConsumer;

/**
 * Receives the undeploy mod's decisions through its "undeploy:events" entrypoint. why? that
 * mod rewrites the local player's glide bit after this logger's H record and hands the
 * client packets that never pass the Connection, so without it a log can't tell what the
 * client saw. Loaded only when undeploy is installed.
 */
public final class UndeployTap implements BiConsumer<String, String> {
    @Override
    public void accept(String kind, String details) {
        NetLog.undeploy(kind, details);
    }
}
