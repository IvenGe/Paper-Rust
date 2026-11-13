package io.papermc.paperrs.host;

import io.papermc.paper.threadedregions.scheduler.GlobalRegionScheduler;
import io.papermc.paper.threadedregions.scheduler.ScheduledTask;
import java.util.function.Consumer;
import org.bukkit.Bukkit;
import org.bukkit.plugin.IllegalPluginAccessException;
import org.bukkit.plugin.Plugin;
import org.bukkit.scheduler.BukkitTask;

/**
 * Folia's global region scheduler for a server with one main thread: tasks run on the main
 * thread through the Bukkit scheduler, as on Paper.
 */
public final class HostGlobalRegionScheduler implements GlobalRegionScheduler {

    @Override
    public void execute(final Plugin plugin, final Runnable run) {
        this.run(plugin, task -> run.run());
    }

    @Override
    public ScheduledTask run(final Plugin plugin, final Consumer<ScheduledTask> task) {
        return this.runDelayed(plugin, task, 1);
    }

    @Override
    public ScheduledTask runDelayed(final Plugin plugin, final Consumer<ScheduledTask> task, final long delayTicks) {
        if (delayTicks <= 0) {
            throw new IllegalArgumentException("Delay ticks may not be <= 0");
        }
        return this.schedule(plugin, task, delayTicks, -1);
    }

    @Override
    public ScheduledTask runAtFixedRate(final Plugin plugin, final Consumer<ScheduledTask> task, final long initialDelayTicks, final long periodTicks) {
        if (initialDelayTicks <= 0 || periodTicks <= 0) {
            throw new IllegalArgumentException("Delay and period ticks may not be <= 0");
        }
        return this.schedule(plugin, task, initialDelayTicks, periodTicks);
    }

    @Override
    public void cancelTasks(final Plugin plugin) {
        // Tasks are Bukkit tasks; Bukkit's scheduler cancels them with the plugin's other tasks.
        Bukkit.getScheduler().cancelTasks(plugin);
    }

    private ScheduledTask schedule(final Plugin plugin, final Consumer<ScheduledTask> consumer, final long delay, final long period) {
        if (!plugin.isEnabled()) {
            throw new IllegalPluginAccessException("Plugin attempted to register task while disabled");
        }
        final Task task = new Task(plugin, period > 0);
        final Runnable body = () -> {
            task.state = ScheduledTask.ExecutionState.RUNNING;
            try {
                consumer.accept(task);
            } finally {
                if (task.state == ScheduledTask.ExecutionState.RUNNING) {
                    task.state = task.repeating ? ScheduledTask.ExecutionState.IDLE : ScheduledTask.ExecutionState.FINISHED;
                } else if (task.state == ScheduledTask.ExecutionState.CANCELLED_RUNNING) {
                    task.state = ScheduledTask.ExecutionState.CANCELLED;
                }
            }
        };
        task.bukkit = period > 0 ? Bukkit.getScheduler().runTaskTimer(plugin, body, delay, period) : Bukkit.getScheduler().runTaskLater(plugin, body, delay);
        return task;
    }

    private static final class Task implements ScheduledTask {
        private final Plugin plugin;
        private final boolean repeating;
        private volatile ExecutionState state = ExecutionState.IDLE;
        private volatile BukkitTask bukkit;

        Task(final Plugin plugin, final boolean repeating) {
            this.plugin = plugin;
            this.repeating = repeating;
        }

        @Override
        public Plugin getOwningPlugin() {
            return this.plugin;
        }

        @Override
        public boolean isRepeatingTask() {
            return this.repeating;
        }

        @Override
        public CancelledState cancel() {
            final ExecutionState current = this.state;
            switch (current) {
                case CANCELLED, CANCELLED_RUNNING:
                    return CancelledState.CANCELLED_ALREADY;
                case FINISHED:
                    return CancelledState.ALREADY_EXECUTED;
                case RUNNING:
                    this.bukkit.cancel();
                    this.state = ExecutionState.CANCELLED_RUNNING;
                    return this.repeating ? CancelledState.NEXT_RUNS_CANCELLED : CancelledState.RUNNING;
                default:
                    this.bukkit.cancel();
                    this.state = ExecutionState.CANCELLED;
                    return CancelledState.CANCELLED_BY_CALLER;
            }
        }

        @Override
        public ExecutionState getExecutionState() {
            return this.state;
        }
    }
}
