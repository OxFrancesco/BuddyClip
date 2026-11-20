package dev.buddyclip.app

import android.app.Activity
import android.os.Bundle
import android.widget.Toast

/**
 * Invisible focus trampoline invoked by the "Copy Latest" notification action.
 * Ensures copying works even on OEM builds that restrict background setPrimaryClip().
 */
class CopyClipActivity : Activity() {
    private var handled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || handled) return
        handled = true

        val latest = SyncRepository.latestRemoteClip()
        val msg = if (latest == null) {
            "No clip received yet"
        } else if (SyncRepository.writeClipToSystemClipboard(this, latest)) {
            "Copied from ${latest.originName}"
        } else {
            "Could not write to clipboard"
        }
        Toast.makeText(applicationContext, msg, Toast.LENGTH_SHORT).show()
        finish()
    }
}
