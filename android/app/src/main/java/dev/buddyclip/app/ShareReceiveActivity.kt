package dev.buddyclip.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * Invisible target for:
 *  - Android Share Sheet (`ACTION_SEND` for plain text and images)
 *  - Android Text Selection floating toolbar (`ACTION_PROCESS_TEXT` -> "Send to PC")
 */
class ShareReceiveActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val outcome = handleIntent(intent)
        val msg = when (outcome) {
            is SendClipOutcome.Sent -> "Sent to ${outcome.targetName}"
            SendClipOutcome.Empty -> "Nothing to send"
            SendClipOutcome.Unchanged -> "Already synced"
            SendClipOutcome.SensitiveSkipped -> "Skipped sensitive content"
            is SendClipOutcome.Failed -> outcome.reason
        }
        Toast.makeText(applicationContext, msg, Toast.LENGTH_SHORT).show()
        finish()
    }

    private fun handleIntent(intent: Intent?): SendClipOutcome {
        if (intent == null) return SendClipOutcome.Empty
        return when (intent.action) {
            Intent.ACTION_PROCESS_TEXT -> {
                val text = intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
                if (text.isNullOrEmpty()) SendClipOutcome.Empty else SyncRepository.sendDirectText(text)
            }
            Intent.ACTION_SEND -> {
                val type = intent.type.orEmpty().lowercase()
                if (type.startsWith("image/")) {
                    val uri: Uri? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                    } else {
                        @Suppress("DEPRECATION")
                        intent.getParcelableExtra(Intent.EXTRA_STREAM)
                    }
                    if (uri == null) {
                        SendClipOutcome.Empty
                    } else {
                        SyncRepository.sendImageUri(this, uri, explicitMime = type)
                    }
                } else {
                    val text = intent.getStringExtra(Intent.EXTRA_TEXT)
                        ?: intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
                    if (text.isNullOrEmpty()) SendClipOutcome.Empty else SyncRepository.sendDirectText(text)
                }
            }
            else -> SendClipOutcome.Empty
        }
    }
}
