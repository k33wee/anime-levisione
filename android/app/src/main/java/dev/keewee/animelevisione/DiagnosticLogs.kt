@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import androidx.core.content.FileProvider
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.source.LoadEventInfo
import androidx.media3.exoplayer.source.MediaLoadData
import java.io.IOException
import java.net.SocketTimeoutException
import java.net.ConnectException
import java.net.UnknownHostException
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.anime_levisione_core.*

/** No messages, URLs, media identifiers, device identifiers or arbitrary stack traces. */
object DiagnosticLogs {
    private val queue = ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(256),
        { task -> Thread(task, "diagnostics").apply { isDaemon = true } },
    )
    private val sessions = AtomicLong()
    @Volatile var available = false
        private set

    fun initialize(context: Context) {
        available = runCatching {
            diagnosticsInitialize(context.filesDir.resolve("diagnostics").absolutePath)
        }.isSuccess
        if (!available) return
        record(DiagnosticEvent.AndroidEnvironment(Build.VERSION.SDK_INT.toUInt()))
        val previous = Thread.getDefaultUncaughtExceptionHandler() ?: return
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            // Write synchronously on the crashing thread, not into a queue that may die.
            runCatching { recordCrash(error) }
            previous.uncaughtException(thread, error)
        }
    }

    fun nextSession() = sessions.incrementAndGet().toULong()

    fun record(event: DiagnosticEvent) {
        if (available) runCatching {
            queue.execute { runCatching { diagnosticsRecord(event) } }
        }
    }

    fun failure(error: Throwable): DiagnosticFailure {
        val causes = generateSequence(error) { it.cause }.take(8).toList()
        return when {
            causes.any { it is SocketTimeoutException } -> DiagnosticFailure.TIMEOUT
            causes.any { it is ConnectException || it is UnknownHostException } -> DiagnosticFailure.CONNECT
            causes.any { it is HttpDataSource.InvalidResponseCodeException } -> DiagnosticFailure.HTTP
            causes.any { it is IOException } -> DiagnosticFailure.IO
            causes.any { it is OutOfMemoryError } -> DiagnosticFailure.MEMORY
            causes.any { it is IllegalStateException } -> DiagnosticFailure.STATE
            causes.any { it is IllegalArgumentException } -> DiagnosticFailure.ARGUMENT
            causes.any { it is SecurityException } -> DiagnosticFailure.SECURITY
            else -> DiagnosticFailure.OTHER
        }
    }

    internal fun recordCrash(error: Throwable) {
        diagnosticsRecord(DiagnosticEvent.AndroidCrash(failure(error)))
        // Whitelisted source components + line numbers, never exception messages.
        generateSequence(error) { it.cause }.take(8).flatMap { it.stackTrace.asSequence() }
            .filter { it.className.startsWith("dev.keewee.animelevisione.") }
            .take(12).forEach { frame ->
                val component = when (frame.fileName) {
                    "PlaybackScreen.kt" -> DiagnosticComponent.PLAYBACK
                    "CatalogViewModel.kt", "BrowseScreen.kt" -> DiagnosticComponent.CATALOG
                    "AppRepository.kt" -> DiagnosticComponent.REPOSITORY
                    "OfflineLibrary.kt" -> DiagnosticComponent.OFFLINE
                    "MainActivity.kt", "AnimeApplication.kt" -> DiagnosticComponent.MAIN
                    "DiagnosticLogs.kt" -> DiagnosticComponent.DIAGNOSTICS
                    else -> DiagnosticComponent.OTHER
                }
                diagnosticsRecord(DiagnosticEvent.AndroidCrashFrame(component, frame.lineNumber.coerceAtLeast(0).toUInt()))
            }
    }

    /** Drain earlier events and snapshot under the Rust writer's lock, off the UI thread. */
    suspend fun shareIntent(context: Context): Intent = withContext(Dispatchers.IO) {
        check(available) { "Diagnostics unavailable" }
        val snapshot = queue.submit<String> {
            diagnosticsRecord(DiagnosticEvent.ExportRequested)
            diagnosticsSnapshot()
        }.get()
        createShareIntent(context, snapshot)
    }

    @Synchronized
    private fun createShareIntent(context: Context, snapshot: String): Intent {
        val directory = context.cacheDir.resolve("diagnostic-exports")
        check(directory.isDirectory || directory.mkdirs())
        // Unique snapshots cannot change while another app is reading them. Bound cache too.
        directory.listFiles()?.sortedByDescending { it.lastModified() }?.drop(2)?.forEach { it.delete() }
        val file = directory.resolve("anime-levisione-diagnostics-${UUID.randomUUID()}.jsonl")
        file.writeText(snapshot)
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.diagnostics", file)
        return Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_STREAM, uri)
            clipData = ClipData.newUri(context.contentResolver, "Diagnostic logs", uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }
}

/** Technical playback telemetry only; deliberately never inspects DataSpec or MediaItem. */
internal class DiagnosticAnalytics(private val session: ULong) : AnalyticsListener {
    private var bufferingAt: Long? = null

    override fun onPlaybackStateChanged(eventTime: AnalyticsListener.EventTime, state: Int) {
        if (state == Player.STATE_BUFFERING && bufferingAt == null) {
            bufferingAt = SystemClock.elapsedRealtime()
            DiagnosticLogs.record(DiagnosticEvent.PlayerBuffering(session, eventTime.totalBufferedDurationMs.coerceAtLeast(0).toULong()))
        } else if (state == Player.STATE_READY) {
            val stall = bufferingAt?.let { SystemClock.elapsedRealtime() - it } ?: 0
            bufferingAt = null
            DiagnosticLogs.record(DiagnosticEvent.PlayerReady(session, stall.coerceAtLeast(0).toULong()))
        }
    }

    override fun onPlayerError(eventTime: AnalyticsListener.EventTime, error: PlaybackException) {
        DiagnosticLogs.record(DiagnosticEvent.PlayerFailure(session, error.errorCode.coerceAtLeast(0).toUInt(), DiagnosticLogs.failure(error)))
    }

    override fun onLoadError(eventTime: AnalyticsListener.EventTime, loadEventInfo: LoadEventInfo, mediaLoadData: MediaLoadData, error: IOException, wasCanceled: Boolean) {
        val status = generateSequence<Throwable>(error) { it.cause }.take(8)
            .filterIsInstance<HttpDataSource.InvalidResponseCodeException>().firstOrNull()?.responseCode ?: 0
        DiagnosticLogs.record(DiagnosticEvent.MediaLoadFailure(session, status.coerceIn(0, 65535).toUShort(), DiagnosticLogs.failure(error), wasCanceled))
    }

    override fun onBandwidthEstimate(eventTime: AnalyticsListener.EventTime, totalLoadTimeMs: Int, totalBytesLoaded: Long, bitrateEstimate: Long) {
        DiagnosticLogs.record(DiagnosticEvent.Bandwidth(session, bitrateEstimate.coerceAtLeast(0).toULong()))
    }
}
