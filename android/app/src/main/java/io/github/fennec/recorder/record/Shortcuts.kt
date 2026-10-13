package io.github.fennec.recorder.record

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.widget.RemoteViews
import io.github.fennec.recorder.MainActivity
import io.github.fennec.recorder.R

/** Opens Fennec Recorder and starts recording (the microphone needs the app in front). */
fun recordIntent(context: Context): Intent =
    Intent(context, MainActivity::class.java)
        .setAction(MainActivity.ACTION_RECORD)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)

private fun recordPending(context: Context): PendingIntent =
    PendingIntent.getActivity(context, 0, recordIntent(context), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

/** Quick settings tile: Record, or Stop while recording. */
class RecordTileService : TileService() {
    override fun onStartListening() {
        val recording = Recorder.state.value is Recorder.Status.Recording
        qsTile?.apply {
            state = if (recording) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            label = if (recording) "Stop recording" else "Record"
            updateTile()
        }
    }

    override fun onClick() {
        if (Recorder.state.value is Recorder.Status.Recording) {
            Recorder.stop(this)
            onStartListening()
            return
        }
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(recordPending(this))
        } else {
            @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(recordIntent(this))
        }
    }
}

/** A home screen button that starts a recording. */
class RecordWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        val views = RemoteViews(context.packageName, R.layout.widget_record).apply {
            setOnClickPendingIntent(R.id.widget_record, recordPending(context))
        }
        ids.forEach { manager.updateAppWidget(it, views) }
    }
}
