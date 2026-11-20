package dev.buddyclip.app

import android.app.Activity
import android.os.Bundle
import android.widget.Toast

/**
 * Invisible 50ms window-focus trampoline invoked by the Quick Settings Tile
 * and the persistent notification's "Send Clipboard" action.
 *
 * Android 10+ requires window focus to read ClipboardManager.primaryClip; waiting for
 * onWindowFocusChanged(true) lets us legally read the clipboard and push it over
 * the encrypted tailnet channel with a single tap and zero UI disruption.
 */
class SendClipboardActivity : Activity() {
    private var handled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || handled) return
        handled = true

        val outcome = SyncRepository.readSystemClipboardAndSend(this, skipIfUnchanged = false)
        val toastText = when (outcome) {
            is SendClipOutcome.Sent -> "Sent to ${outcome.targetName}"
            SendClipOutcome.Unchanged -> "Already synced"
            SendClipOutcome.Empty -> "Clipboard is empty"
            SendClipOutcome.SensitiveSkipped -> "Skipped sensitive clipboard item"
            is SendClipOutcome.Failed -> outcome.reason
        }
        Toast.makeText(applicationContext, toastText, Toast.LENGTH_SHORT).show()
        finish()
    }
}
