@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.app.Notification
import android.content.Context
import androidx.core.app.NotificationCompat
import androidx.media3.common.TrackSelectionParameters
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.offline.DefaultDownloadIndex
import androidx.media3.exoplayer.offline.DefaultDownloaderFactory
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadHelper
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.exoplayer.offline.DownloaderFactory
import androidx.media3.exoplayer.scheduler.Requirements
import androidx.media3.exoplayer.scheduler.Scheduler
import java.io.IOException
import java.util.concurrent.Executors
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import uniffi.anime_levisione_core.StreamInfo

data class SavedEpisode(val download: Download, val metadata: DownloadMetadata)

class OfflineLibrary(private val context: Context) {
    private val database = StandaloneDatabaseProvider(context)
    private val cache =
        SimpleCache(context.filesDir.resolve("offline"), NoOpCacheEvictor(), database)
    private val executor = Executors.newFixedThreadPool(3)
    private val replacements = mutableMapOf<String, DownloadRequest>()
    private val _downloads = MutableStateFlow<List<SavedEpisode>>(emptyList())
    val downloads = _downloads.asStateFlow()
    val manager =
        DownloadManager(
                context,
                DefaultDownloadIndex(database),
                DownloaderFactory { request ->
                    val metadata = DownloadMetadata.parse(request.data)
                    DefaultDownloaderFactory(cacheFactory(metadata.referer, false), executor)
                        .createDownloader(request)
                },
            )
            .apply {
                maxParallelDownloads = 1
                addListener(
                    object : DownloadManager.Listener {
                        override fun onInitialized(downloadManager: DownloadManager) {
                            refresh()
                        }

                        override fun onDownloadChanged(
                            downloadManager: DownloadManager,
                            download: Download,
                            finalException: Exception?,
                        ) {
                            refresh()
                        }

                        override fun onDownloadRemoved(
                            downloadManager: DownloadManager,
                            download: Download,
                        ) {
                            // Finish removing the old signed manifest/segments before replacing
                            // its request: merging requests would retain stale cache and
                            // renditions.
                            replacements.remove(download.request.id)?.let { request ->
                                downloadManager.addDownload(request)
                                downloadManager.resumeDownloads()
                            }
                            refresh()
                        }
                    }
                )
            }

    fun httpFactory(referer: String) =
        DefaultDataSource.Factory(
            context,
            DefaultHttpDataSource.Factory()
                .setUserAgent("Mozilla/5.0 (anime-levisione Android)")
                .setConnectTimeoutMs(15000)
                .setReadTimeoutMs(30000)
                .setDefaultRequestProperties(
                    if (referer.isBlank()) emptyMap() else mapOf("Referer" to referer)
                ),
        )

    fun cacheFactory(referer: String, offlineOnly: Boolean): CacheDataSource.Factory =
        CacheDataSource.Factory()
            .setCache(cache)
            .setUpstreamDataSourceFactory(if (offlineOnly) null else httpFactory(referer))

    fun wifiOnly(enabled: Boolean) {
        manager.requirements =
            Requirements(Requirements.NETWORK or if (enabled) Requirements.NETWORK_UNMETERED else 0)
    }

    fun refresh() {
        val list = mutableListOf<SavedEpisode>()
        manager.downloadIndex.getDownloads().use { cursor ->
            while (cursor.moveToNext()) {
                val download = cursor.download
                runCatching { DownloadMetadata.parse(download.request.data) }
                    .getOrNull()
                    ?.let { list += SavedEpisode(download, it) }
            }
        }
        _downloads.value = list
    }

    suspend fun prepare(
        metadata: DownloadMetadata,
        stream: StreamInfo,
        height: Int,
    ): DownloadRequest =
        withContext(Dispatchers.Main) {
            check(
                android.os.StatFs(context.filesDir.absolutePath).availableBytes > 250L * 1024 * 1024
            ) {
                "Less than 250 MB free. Free some space before downloading an episode."
            }
            val item = stream.mediaItem()
            val parameters =
                TrackSelectionParameters.Builder(context)
                    .setPreferredAudioLanguage(metadata.language)
                    .setMaxVideoSize(height * 16 / 9, height)
                    .setForceHighestSupportedBitrate(true)
                    .build()
            val helper =
                DownloadHelper.forMediaItem(
                    item,
                    parameters,
                    DefaultRenderersFactory(context),
                    httpFactory(stream.referer),
                )
            suspendCancellableCoroutine { continuation ->
                continuation.invokeOnCancellation {
                    android.os.Handler(android.os.Looper.getMainLooper()).post { helper.release() }
                }
                helper.prepare(
                    object : DownloadHelper.Callback {
                        override fun onPrepared(helper: DownloadHelper) {
                            if (continuation.isActive)
                                continuation.resume(
                                    helper.getDownloadRequest(
                                        metadata.episode.key(metadata.anime),
                                        metadata.bytes(),
                                    )
                                )
                            helper.release()
                        }

                        override fun onPrepareError(helper: DownloadHelper, e: IOException) {
                            if (continuation.isActive) continuation.resumeWithException(e)
                            helper.release()
                        }
                    }
                )
            }
        }

    fun add(request: DownloadRequest) {
        val old = _downloads.value.firstOrNull { it.download.request.id == request.id }?.download
        if (
            old?.state == Download.STATE_FAILED &&
                (old.request.uri != request.uri || old.request.streamKeys != request.streamKeys)
        ) {
            replacements[request.id] = request
            DownloadService.sendRemoveDownload(
                context,
                EpisodeDownloadService::class.java,
                request.id,
                true,
            )
        } else {
            DownloadService.sendAddDownload(
                context,
                EpisodeDownloadService::class.java,
                request,
                true,
            )
            DownloadService.sendResumeDownloads(context, EpisodeDownloadService::class.java, true)
        }
    }

    fun remove(id: String) {
        replacements.remove(id)
        DownloadService.sendRemoveDownload(context, EpisodeDownloadService::class.java, id, false)
    }

    fun pause() =
        DownloadService.sendPauseDownloads(context, EpisodeDownloadService::class.java, false)

    fun resume() =
        DownloadService.sendResumeDownloads(context, EpisodeDownloadService::class.java, true)
}

class EpisodeDownloadService :
    DownloadService(47, 1000L, "episode-downloads", R.string.download_channel, 0) {
    override fun getDownloadManager() = (application as AnimeApplication).offline.manager

    override fun getScheduler(): Scheduler? = null

    override fun getForegroundNotification(
        downloads: MutableList<Download>,
        notMetRequirements: Int,
    ): Notification {
        val active =
            downloads.count {
                it.state == Download.STATE_DOWNLOADING || it.state == Download.STATE_QUEUED
            }
        return NotificationCompat.Builder(this, "episode-downloads")
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("Saving episodes")
            .setContentText(
                if (notMetRequirements != 0) "Waiting for the selected network"
                else "$active download(s) • anime-levisione"
            )
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }
}
