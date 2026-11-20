package dev.buddyclip.app

import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * Quick Settings Tile ("Send to PC"): swipe down anywhere on your phone and tap once
 * to encrypt and push your current clipboard straight to your PC over Tailscale.
 */
class BuddyClipTileService : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        refreshTile()
    }

    override fun onClick() {
        super.onClick()
        val intent = Intent(this, SendClipboardActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val pending = PendingIntent.getActivity(
                this,
                301,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            startActivityAndCollapse(pending)
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }

    private fun refreshTile() {
        val tile = qsTile ?: return
        val conn = SyncRepository.connection.value
        val cfg = SyncRepository.config.value

        tile.state = when (conn.phase) {
            ConnectionPhase.CONNECTED -> Tile.STATE_ACTIVE
            ConnectionPhase.UNPAIRED -> Tile.STATE_UNAVAILABLE
            else -> Tile.STATE_INACTIVE
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.subtitle = when (conn.phase) {
                ConnectionPhase.CONNECTED -> cfg.hostName.ifBlank { "Connected" }
                ConnectionPhase.CONNECTING -> "Connecting…"
                ConnectionPhase.UNPAIRED -> "Not paired"
                else -> "Tap to send"
            }
        }
        tile.updateTile()
    }

    companion object {
        fun requestUpdate(context: Context) {
            runCatching {
                requestListeningState(
                    context,
                    ComponentName(context, BuddyClipTileService::class.java),
                )
            }
        }
    }
}
