package io.papermc.paperrs.host.entity;

import io.papermc.paper.threadedregions.scheduler.EntityScheduler;
import io.papermc.paper.threadedregions.scheduler.ScheduledTask;
import java.util.function.Consumer;
import org.bukkit.Bukkit;
import org.bukkit.entity.Entity;
import org.bukkit.plugin.IllegalPluginAccessException;
import org.bukkit.plugin.Plugin;
import org.bukkit.scheduler.BukkitTask;
import org.jetbrains.annotations.Nullable;

/**
 * Folia's entity scheduler ({@code Entity.getScheduler()}) for a server with one main thread:
 * tasks run on the main thread through the Bukkit scheduler while the entity is valid. Once the
 * entity is gone (removed, dead, or a player who left), a task does not run again; its
 * {@code retired} callback runs once instead, as on Paper.
 */
public final class HostEntityScheduler implements EntityScheduler {
    private final Entity entity;

    public HostEntityScheduler(final Entity entity) {
        this.entity = entity;
    }

    private boolean retired() {
        return !this.entity.isValid();
    }

    @Override
    public boolean execute(final Plugin plugin, final Runnable run, final @Nullable Runnable retired, final long delay) {
        if (delay < 1) {
            throw new IllegalArgumentException("Delay ticks may not be <= 0");
        }
        if (this.retired()) {
            return false;
        }
        this.schedule(plugin, task -> run.run(), retired, delay, -1);
        return true;
    }

    @Override
    public @Nullable ScheduledTask run(final Plugin plugin, final Consumer<ScheduledTask> task, final @Nullable Runnable retired) {
        return this.runDelayed(plugin, task, retired, 1);
    }

    @Override
    public @Nullable ScheduledTask runDelayed(final Plugin plugin, final Consumer<ScheduledTask> task, final @Nullable Runnable retired, final long delayTicks) {
        if (delayTicks <= 0) {
            throw new IllegalArgumentException("Delay ticks may not be <= 0");
        }
        return this.retired() ? null : this.schedule(plugin, task, retired, delayTicks, -1);
    }

    @Override
    public @Nullable ScheduledTask runAtFixedRate(final Plugin plugin, final Consumer<ScheduledTask> task, final @Nullable Runnable retired, final long initialDelayTicks, final long periodTicks) {
        if (initialDelayTicks <= 0 || periodTicks <= 0) {
            throw new IllegalArgumentException("Delay and period ticks may not be <= 0");
        }
        return this.retired() ? null : this.schedule(plugin, task, retired, initialDelayTicks, periodTicks);
    }

    private Task schedule(final Plugin plugin, final Consumer<ScheduledTask> consumer, final @Nullable Runnable retired, final long delay, final long period) {
        if (!plugin.isEnabled()) {
            throw new IllegalPluginAccessException("Plugin attempted to register task while disabled");
        }
        final Task task = new Task(plugin, period > 0);
        final Runnable body = () -> {
            if (task.state == ScheduledTask.ExecutionState.CANCELLED) {
                return;
            }
            if (this.retired()) {
                task.state = ScheduledTask.ExecutionState.CANCELLED;
                task.bukkit.cancel();
                if (retired != null) {
                    retired.run();
                }
                return;
            }
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
            switch (this.state) {
                case IDLE -> {
                    this.state = ExecutionState.CANCELLED;
                    this.bukkit.cancel();
                    return CancelledState.CANCELLED_BY_CALLER;
                }
                case RUNNING -> {
                    this.state = ExecutionState.CANCELLED_RUNNING;
                    this.bukkit.cancel();
                    return this.repeating ? CancelledState.NEXT_RUNS_CANCELLED : CancelledState.RUNNING;
                }
                case CANCELLED, CANCELLED_RUNNING -> {
                    return this.repeating ? CancelledState.NEXT_RUNS_CANCELLED_ALREADY : CancelledState.CANCELLED_ALREADY;
                }
                default -> {
                    return CancelledState.ALREADY_EXECUTED;
                }
            }
        }

        @Override
        public ExecutionState getExecutionState() {
            return this.state;
        }
    }
}
