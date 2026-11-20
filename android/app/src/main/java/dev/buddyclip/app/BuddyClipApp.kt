package dev.buddyclip.app

import android.app.Application

class BuddyClipApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SyncRepository.init(this)
    }
}
