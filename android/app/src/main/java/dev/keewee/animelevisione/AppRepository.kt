package dev.keewee.animelevisione

import android.content.Context
import android.net.Uri
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import uniffi.anime_levisione_core.Anime
import uniffi.anime_levisione_core.AnimeLib
import uniffi.anime_levisione_core.Episode
import uniffi.anime_levisione_core.ProviderId
import uniffi.anime_levisione_core.StreamInfo

// One core/library lock per application, not per Activity or recomposition.
class AppRepository(private val context: Context) {
    private val initialization = Mutex()
    private var library: AnimeLib? = null
    val preferences = context.getSharedPreferences("preferences", Context.MODE_PRIVATE)

    suspend fun <T> call(block: (AnimeLib) -> T): T =
        withContext(Dispatchers.IO) {
            val core =
                initialization.withLock {
                    library
                        ?: AnimeLib.withLibrary(context.filesDir.resolve("library").absolutePath)
                            .also { library = it }
                }
            block(core)
        }
}

// VixCloud uses extensionless /playlist/ URLs; URI-only inference treats them
// as progressive files, so neither HLS playback nor language discovery works.
fun StreamInfo.mediaItem(): MediaItem {
    val path = Uri.parse(url).path.orEmpty().lowercase()
    val mime =
        if (path.endsWith(".m3u8") || path.contains("/playlist/")) MimeTypes.APPLICATION_M3U8
        else null
    return MediaItem.Builder().setUri(url).setMimeType(mime).build()
}

fun Anime.key() = "${provider.name}:$id"

fun Episode.key(anime: Anime) = "${anime.key()}:$id"

fun ProviderId.label() =
    when (this) {
        ProviderId.ANIME_WORLD -> "AnimeWorld"
        ProviderId.ANIME_UNITY -> "AnimeUnity"
        ProviderId.STREAMING_COMMUNITY -> "StreamingCommunity"
    }

fun Anime.audioLabel() =
    if (provider == ProviderId.STREAMING_COMMUNITY) "MULTI AUDIO"
    else if (dub) "ITA DUB" else "SUB ITA"

// Download metadata contains only library records and the necessary HTTP referer.
// The download index is in app-private storage, never shared or logged.
data class DownloadMetadata(
    val anime: Anime,
    val episode: Episode,
    val referer: String,
    val language: String,
) {
    fun bytes(): ByteArray =
        JSONObject()
            .put("anime", anime.json())
            .put(
                "episode",
                JSONObject()
                    .put("id", episode.id)
                    .put("number", episode.number)
                    .put("title", episode.title),
            )
            .put("referer", referer)
            .put("language", language)
            .toString()
            .toByteArray(Charsets.UTF_8)

    companion object {
        fun parse(bytes: ByteArray): DownloadMetadata {
            val v = JSONObject(bytes.toString(Charsets.UTF_8))
            val a = v.getJSONObject("anime")
            val e = v.getJSONObject("episode")
            return DownloadMetadata(
                Anime(
                    provider = ProviderId.valueOf(a.getString("provider")),
                    id = a.getString("id"),
                    title = a.getString("title"),
                    aliases = a.strings("aliases"),
                    synopsis = a.optString("synopsis"),
                    year = a.optString("year"),
                    kind = a.optString("kind"),
                    status = a.optString("status"),
                    score = a.optString("score"),
                    episodes = a.optLong("episodes").toUInt(),
                    dub = a.optBoolean("dub"),
                    genres = a.strings("genres"),
                    pageUrl = a.optString("pageUrl"),
                    coverUrl = a.optString("coverUrl"),
                ),
                Episode(e.getString("id"), e.getString("number"), e.optString("title")),
                v.optString("referer"),
                v.optString("language", "it"),
            )
        }
    }
}

private fun JSONObject.strings(key: String): List<String> =
    optJSONArray(key)?.let { a -> List(a.length()) { a.getString(it) } }.orEmpty()

private fun Anime.json() =
    JSONObject()
        .put("provider", provider.name)
        .put("id", id)
        .put("title", title)
        .put("aliases", JSONArray(aliases))
        .put("synopsis", synopsis)
        .put("year", year)
        .put("kind", kind)
        .put("status", status)
        .put("score", score)
        .put("episodes", episodes.toLong())
        .put("dub", dub)
        .put("genres", JSONArray(genres))
        .put("pageUrl", pageUrl)
        .put("coverUrl", coverUrl)
