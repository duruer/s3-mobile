// Settlers 3 Port: foreground service that keeps the process running while a game continues in the background
// (docs/porting/android.md "Background play"). Started and fed from Rust (bgservice.rs) with plain intents:
//   START  (startForegroundService) extra game_ms: ongoing "running" notification with a chronometer
//   UPDATE (startService)           extra game_ms: re-sync the chronometer (game speed, in-game pause)
//   EVENT  (startService)           extra text: one "Game events" notification (lines separated by '\n')
//   stopService: removes the ongoing notification and the event notifications.
// SPDX-License-Identifier: GPL-3.0-or-later
package com.settlers3port.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import android.os.PowerManager;

public class GameService extends Service {
    static final String ACTION_START = "com.settlers3port.app.START";
    static final String ACTION_UPDATE = "com.settlers3port.app.UPDATE";
    static final String ACTION_EVENT = "com.settlers3port.app.EVENT";
    static final String CH_RUNNING = "running";
    // "events" (first version) had no vibration; a channel's settings cannot change once created, so it is
    // replaced by "events_vibrate" (deleted in channels()).
    static final String CH_EVENTS_OLD = "events";
    static final String CH_EVENTS = "events_vibrate";
    // Same as CH_EVENTS without vibration (setting "Vibration" in the game menus).
    static final String CH_EVENTS_QUIET = "events_quiet";
    static final String GROUP_EVENTS = "com.settlers3port.app.EVENTS";
    static final int ID_RUNNING = 1;
    static final int ID_EVENTS_FIRST = 100;

    private PowerManager.WakeLock wakeLock;
    private int nextEventId = ID_EVENTS_FIRST;
    private long gameMs;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        channels();
        if (action == null || ACTION_START.equals(action)) {
            gameMs = intent != null ? intent.getLongExtra("game_ms", 0) : 0;
            Notification n = running();
            if (Build.VERSION.SDK_INT >= 34) {
                startForeground(ID_RUNNING, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
            } else {
                startForeground(ID_RUNNING, n);
            }
            if (wakeLock == null) {
                PowerManager pm = (PowerManager) getSystemService(POWER_SERVICE);
                wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "s3port:game");
                wakeLock.setReferenceCounted(false);
                wakeLock.acquire();
            }
        } else if (ACTION_UPDATE.equals(action)) {
            gameMs = intent.getLongExtra("game_ms", gameMs);
            nm().notify(ID_RUNNING, running());
        } else if (ACTION_EVENT.equals(action)) {
            String text = intent.getStringExtra("text");
            if (text != null && !text.isEmpty()) {
                event(text, intent.getLongExtra("vibrate", 1) != 0);
            }
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onTaskRemoved(Intent rootIntent) {
        // Swiped away from the recent apps: the user closed the game.
        stopSelf();
        android.os.Process.killProcess(android.os.Process.myPid());
    }

    @Override
    public void onDestroy() {
        if (wakeLock != null) {
            wakeLock.release();
            wakeLock = null;
        }
        stopForeground(STOP_FOREGROUND_REMOVE);
        NotificationManager nm = nm();
        for (int id = ID_EVENTS_FIRST; id < nextEventId; id++) {
            nm.cancel(id);
        }
        super.onDestroy();
    }

    private NotificationManager nm() {
        return (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
    }

    private void channels() {
        NotificationManager nm = nm();
        if (nm.getNotificationChannel(CH_RUNNING) == null) {
            NotificationChannel c = new NotificationChannel(CH_RUNNING, "Game running", NotificationManager.IMPORTANCE_LOW);
            c.setDescription("Shown while a game keeps running in the background");
            c.setShowBadge(false);
            nm.createNotificationChannel(c);
        }
        if (nm.getNotificationChannel(CH_EVENTS_QUIET) == null) {
            NotificationChannel c = new NotificationChannel(CH_EVENTS_QUIET, "Game events (no vibration)", NotificationManager.IMPORTANCE_DEFAULT);
            c.setDescription("Game messages while in the background, when vibration is off in the game menu");
            c.enableVibration(false);
            c.setVibrationPattern(new long[] {0});
            nm.createNotificationChannel(c);
        }
        if (nm.getNotificationChannel(CH_EVENTS) == null) {
            NotificationChannel c = new NotificationChannel(CH_EVENTS, "Game events", NotificationManager.IMPORTANCE_DEFAULT);
            c.setDescription("Messages of the game (e.g. resources found) while it runs in the background");
            c.enableVibration(true);
            nm.createNotificationChannel(c);
        }
        if (nm.getNotificationChannel(CH_EVENTS_OLD) != null) {
            nm.deleteNotificationChannel(CH_EVENTS_OLD);
        }
    }

    private int icon() {
        int id = getResources().getIdentifier("ic_notify", "drawable", getPackageName());
        return id != 0 ? id : android.R.drawable.ic_media_play;
    }

    private PendingIntent openApp() {
        Intent i = getPackageManager().getLaunchIntentForPackage(getPackageName());
        if (i == null) {
            i = new Intent();
            i.setClassName(this, "android.app.NativeActivity");
        }
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED);
        return PendingIntent.getActivity(this, 0, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification running() {
        Notification.Builder b = new Notification.Builder(this, CH_RUNNING);
        if (android.os.Build.VERSION.SDK_INT >= 31) {
            // Android 12+ delays foreground-service notifications by up to 10 s unless asked to show at once.
            b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE);
        }
        return b
            .setSmallIcon(icon())
            .setContentTitle("Settlers 3 Mobile — running")
            .setContentText("Tap to return")
            .setUsesChronometer(true)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis() - gameMs)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(openApp())
            .build();
    }

    private void event(String text, boolean vibrate) {
        String[] lines = text.split("\n");
        Notification.Builder b = new Notification.Builder(this, vibrate ? CH_EVENTS : CH_EVENTS_QUIET)
            .setSmallIcon(icon())
            .setContentTitle(lines.length == 1 ? "Settlers 3 Mobile" : "Settlers 3 Mobile — " + lines.length + " messages")
            .setContentText(lines[lines.length - 1])
            .setAutoCancel(true)
            .setGroup(GROUP_EVENTS)
            .setCategory(Notification.CATEGORY_EVENT)
            .setContentIntent(openApp());
        if (lines.length > 1) {
            Notification.InboxStyle st = new Notification.InboxStyle();
            for (String l : lines) {
                st.addLine(l);
            }
            b.setStyle(st);
        } else {
            b.setStyle(new Notification.BigTextStyle().bigText(text));
        }
        nm().notify(nextEventId++, b.build());
    }
}
