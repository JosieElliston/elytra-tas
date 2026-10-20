package jealoustone.elytratas.netlog;

import java.io.BufferedWriter;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.zip.GZIPOutputStream;

/**
 * Writes lines to a gzip file on its own thread. {@link #offer} never blocks: game and
 * Netty threads only enqueue, so logging can't stall a tick. Lines past the queue bound
 * are dropped and counted in an {@code E dropped} record.
 */
final class LogWriter implements Runnable {
    private static final int QUEUE_LIMIT = 1 << 20;
    private static final long FLUSH_NANOS = TimeUnit.SECONDS.toNanos(1);
    private static final String STOP = new String("stop");

    private final LinkedBlockingQueue<String> queue = new LinkedBlockingQueue<>(QUEUE_LIMIT);
    private final AtomicLong dropped = new AtomicLong();
    private final Writer out;
    private final Thread thread;
    final Path path;

    LogWriter(Path path) throws IOException {
        this.path = path;
        Files.createDirectories(path.getParent());
        // syncFlush: every flush() emits a complete deflate block, so a crash loses at
        // most the last second and the file still decompresses up to that point.
        out = new BufferedWriter(new OutputStreamWriter(new GZIPOutputStream(
                Files.newOutputStream(path, StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE), 1 << 16, true),
                StandardCharsets.UTF_8), 1 << 16);
        thread = new Thread(this, "elytra-tas log writer");
        thread.setDaemon(true);
        thread.start();
    }

    void offer(String line) {
        if (!queue.offer(line)) dropped.incrementAndGet();
    }

    void close() {
        try {
            if (!queue.offer(STOP, 1, TimeUnit.SECONDS)) {
                queue.clear();
                queue.offer(STOP);
            }
            thread.join(5000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    @Override
    public void run() {
        List<String> batch = new ArrayList<>(4096);
        long lastFlush = System.nanoTime();
        try {
            boolean running = true;
            while (running) {
                String first = queue.poll(200, TimeUnit.MILLISECONDS);
                if (first != null) {
                    batch.add(first);
                    queue.drainTo(batch, 4095);
                    for (String line : batch) {
                        if (line == STOP) {
                            running = false;
                            break;
                        }
                        out.write(line);
                        out.write('\n');
                    }
                    batch.clear();
                }
                long now = System.nanoTime();
                if (!running || now - lastFlush >= FLUSH_NANOS) {
                    long lost = dropped.getAndSet(0);
                    if (lost > 0) out.write("E\t" + now + "\t" + NetLog.tick + "\tdropped\t" + lost + "\n");
                    out.flush();
                    lastFlush = now;
                }
            }
        } catch (IOException e) {
            NetLog.LOGGER.error("elytra-tas: log write failed, logging stopped: {}", path, e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            try {
                out.close();
            } catch (IOException e) {
                NetLog.LOGGER.error("elytra-tas: could not close {}", path, e);
            }
        }
    }
}
