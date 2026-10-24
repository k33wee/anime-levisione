package dev.keewee.animelevisione

import android.app.Application
import android.content.Context

class AnimeApplication : Application() {
    val repository by lazy { AppRepository(this) }
    val offline by lazy { OfflineLibrary(this) }

    override fun onCreate() {
        super.onCreate()
        System.loadLibrary("anime_levisione_core")
        // Rustls needs the application context/class loader before HTTPS requests.
        initialize(this)
    }

    private external fun initialize(context: Context)
}
