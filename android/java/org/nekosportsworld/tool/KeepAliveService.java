package org.nekosportsworld.tool;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.os.Build;
import android.os.IBinder;

/**
 * 前台保活服务：与 NativeActivity/Rust 共享同一进程。
 *
 * 只要本服务处于前台（startForeground），整个进程就拥有前台优先级，
 * 避免「跑步提交流程」在后台被 Doze / LMK / 厂商清理杀掉。
 *
 * 注意：这是「保活壳」，不承载业务逻辑；Rust 的跑步/极验流程仍在主 Activity 内运行。
 */
public final class KeepAliveService extends Service {

    private static final String CHANNEL_ID = "keep_alive";
    private static final int NOTIFY_ID = 1001;

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        createChannel();
        Notification notification = new Notification.Builder(this, CHANNEL_ID)
                .setContentTitle("RUN TOOL 后台运行中")
                .setContentText("保持前台运行，避免跑步任务被系统回收")
                .setSmallIcon(android.R.drawable.ic_menu_compass)
                .setOngoing(true)
                .setShowWhen(false)
                .build();

        // startForeground 必须在 startForegroundService 后 5s 内调用，否则 ANR/crash
        // dataSync 类型见 AndroidManifest 的 foregroundServiceType 声明
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFY_ID, notification, android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
        } else {
            startForeground(NOTIFY_ID, notification);
        }

        // START_STICKY：被系统回收后，系统会用 null intent 重新拉起，保持进程存活
        return START_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public void onDestroy() {
        // 组件销毁时退出前台，避免被系统判定「前台服务泄漏」
        stopForeground(true);
        super.onDestroy();
    }

    private void createChannel() {
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    "后台保活",
                    NotificationManager.IMPORTANCE_LOW);
            channel.setDescription("保持进程前台，防止跑步任务被杀");
            channel.setShowBadge(false);
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (manager != null) {
                manager.createNotificationChannel(channel);
            }
        }
    }
}