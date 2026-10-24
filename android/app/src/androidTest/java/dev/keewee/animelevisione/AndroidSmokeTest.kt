@file:androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)

package dev.keewee.animelevisione

import android.content.pm.ActivityInfo
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.drawable.AdaptiveIconDrawable
import android.graphics.drawable.Drawable
import android.net.Uri
import android.os.Build
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelProvider
import androidx.media3.common.MimeTypes
import androidx.media3.exoplayer.offline.Download
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.anime_levisione_core.*

@RunWith(AndroidJUnit4::class)
class AndroidSmokeTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun fixture() =
        Anime(
            ProviderId.ANIME_WORLD,
            "fixture",
            "Regression Fixture",
            emptyList(),
            "Synthetic test media",
            "2024",
            "TV",
            "Finished",
            "",
            2u,
            false,
            emptyList(),
            "",
            "",
        )

    private fun waitForText(text: String) =
        compose.waitUntil(20000) {
            compose.onAllNodesWithText(text, substring = true).fetchSemanticsNodes().isNotEmpty()
        }

    private fun englishDialogReady() =
        compose.onAllNodesWithText("English", substring = true).fetchSemanticsNodes().isNotEmpty()

    private fun playbackStarted() =
        compose.onAllNodesWithText("Playing", substring = true).fetchSemanticsNodes().isNotEmpty()

    /** Multi-audio media must pause for a choice; single-audio media may auto-start. */
    private fun startPlaybackChoosingEnglish() {
        compose.waitUntil(20000) { englishDialogReady() || playbackStarted() }
        if (!playbackStarted()) {
            compose.onAllNodesWithText("English", substring = true).onFirst().performClick()
            waitForText("Playing")
        }
        // Pausing pins the auto-hiding controls, making later assertions stable.
        compose.onNodeWithTag("playPause").performClick()
    }

    private fun copyFixture(name: String) =
        compose.activity.cacheDir.resolve("$name-${UUID.randomUUID()}.mp4").apply {
            InstrumentationRegistry.getInstrumentation().context.assets.open("bilingual.mp4").use {
                input ->
                outputStream().use { input.copyTo(it) }
            }
        }

    @Test
    fun extensionlessHlsIsNeverTreatedAsAProgressiveFile() {
        assertEquals(
            MimeTypes.APPLICATION_M3U8,
            StreamInfo("https://vixcloud.example/playlist/123?token=fixture", "")
                .mediaItem()
                .localConfiguration
                ?.mimeType,
        )
        assertEquals(
            MimeTypes.APPLICATION_M3U8,
            StreamInfo("https://cdn.example/MASTER.M3U8?x=1", "")
                .mediaItem()
                .localConfiguration
                ?.mimeType,
        )
        assertNull(
            StreamInfo("https://cdn.example/video.mp4", "").mediaItem().localConfiguration?.mimeType
        )
    }

    @Test
    fun launchesAndRecreatesWithNativeLibraryLoaded() {
        compose.onNodeWithText("anime-levisione").assertIsDisplayed()
        compose.onNodeWithText("levisione").assertDoesNotExist()
        compose.onNodeWithTag("navSearch").performClick()
        compose.onNodeWithTag("searchField").assertIsDisplayed()
        compose.activityRule.scenario.recreate()
        compose.onNodeWithTag("searchField").assertIsDisplayed()
        AnimeLib().use { assertTrue(it.watchlist().isEmpty()) }
    }

    @Test
    fun launcherIconAndApplicationLabelUseTheNewBrand() {
        val activity = compose.activity
        assertEquals(
            "anime-levisione",
            activity.packageManager.getApplicationLabel(activity.applicationInfo),
        )
        val icon = activity.getDrawable(R.mipmap.ic_launcher)
        assertTrue(
            "The launcher must support the system's adaptive masks",
            icon is AdaptiveIconDrawable,
        )
        icon as AdaptiveIconDrawable
        fun assertNotBlank(drawable: Drawable) {
            val bitmap = Bitmap.createBitmap(192, 192, Bitmap.Config.ARGB_8888)
            try {
                drawable.setBounds(0, 0, 192, 192)
                drawable.draw(Canvas(bitmap))
                val pixels = IntArray(192 * 192)
                bitmap.getPixels(pixels, 0, 192, 0, 0, 192, 192)
                assertTrue(
                    "Icon artwork must render, not just its background",
                    pixels.any { Color.alpha(it) > 0 },
                )
            } finally {
                bitmap.recycle()
            }
        }
        assertNotBlank(icon.foreground)
        if (Build.VERSION.SDK_INT >= 33) {
            val monochrome = icon.monochrome
            assertNotNull("Android 13+ must have a themed icon", monochrome)
            assertNotBlank(monochrome!!)
        }
    }

    @Test
    fun fullBrandNameFitsANarrowHeaderInPrivateMode() {
        compose.activity.runOnUiThread {
            compose.activity.setContent {
                LevisioneTheme {
                    val density = LocalDensity.current
                    CompositionLocalProvider(LocalDensity provides Density(density.density, 1.3f)) {
                        Surface {
                            Box(Modifier.width(320.dp)) { BrandHeader(Destination.Home, true, {}) }
                        }
                    }
                }
            }
        }
        compose.onNodeWithText("anime-levisione").assertIsDisplayed()
        compose.onNodeWithText("Private").assertIsDisplayed()
        compose.onNodeWithTag("brandName").performSemanticsAction(
            SemanticsActions.GetTextLayoutResult
        ) { action ->
            val layouts = mutableListOf<TextLayoutResult>()
            assertTrue(action(layouts))
            assertTrue(layouts.isNotEmpty())
            val layout = layouts.first()
            assertFalse(
                "Clipped: width=${layout.didOverflowWidth}, height=${layout.didOverflowHeight}, size=${layout.size}, paragraph=${layout.multiParagraph.width}x${layout.multiParagraph.height}",
                layout.hasVisualOverflow,
            )
        }
    }

    @Test
    fun watchlistNavigationSurvivesActivityRecreation() {
        val vm = ViewModelProvider(compose.activity)[CatalogViewModel::class.java]
        compose.runOnIdle { vm.toggleWatchlist(fixture()) }
        compose.waitUntil(15000) { vm.library.watchlist.any { it.id == "fixture" } }
        compose.onNodeWithTag("navLibrary").performClick()
        compose.onNodeWithText("Regression Fixture").assertIsDisplayed()
        compose.activityRule.scenario.recreate()
        compose.onNodeWithText("Regression Fixture").assertIsDisplayed()
        compose.runOnIdle { vm.toggleWatchlist(fixture()) }
        compose.waitUntil(15000) { vm.library.watchlist.none { it.id == "fixture" } }
    }

    @Test
    fun audioPickerWaitsBeforePlaybackAndAllowsEnglish() {
        val file = copyFixture("audio-fixture")
        val episode = Episode("fixture-audio", "1", "Bilingual sample")
        val request =
            Playback(
                fixture(),
                episode,
                listOf(episode),
                StreamInfo(Uri.fromFile(file).toString(), ""),
                0.0,
                false,
            )
        val offline = (compose.activity.application as AnimeApplication).offline
        compose.activity.runOnUiThread {
            compose.activity.setContent {
                LevisioneTheme {
                    PlaybackScreen(request, offline, "it", false, {}, { _, _, _, _ -> }, {}, {})
                }
            }
        }
        compose.waitUntil(20000) { englishDialogReady() }
        compose
            .onNodeWithTag("playbackStatus")
            .assertTextContains("Choose audio to start", substring = true)
        compose.onAllNodesWithText("English", substring = true).onFirst().performClick()
        waitForText("Playing")
        compose.waitUntil(20000) {
            compose.onAllNodesWithText("Choose audio").fetchSemanticsNodes().isEmpty()
        }
        compose.activity.runOnUiThread { compose.activity.setContent { Text("Done") } }
        file.delete()
    }

    @Test
    fun downloadedEpisodePlaysAfterOriginalSourceIsDeleted() {
        val file = copyFixture("download-fixture")
        val episode = Episode("offline-${UUID.randomUUID()}", "1", "Offline regression")
        val metadata = DownloadMetadata(fixture(), episode, "", "en")
        val stream = StreamInfo(Uri.fromFile(file).toString(), "")
        val offline = (compose.activity.application as AnimeApplication).offline
        val download = runBlocking { offline.prepare(metadata, stream, 480) }
        compose.runOnIdle { offline.add(download) }
        compose.waitUntil(30000) {
            offline.downloads.value.any {
                it.download.request.id == download.id &&
                    it.download.state == Download.STATE_COMPLETED
            }
        }
        assertTrue(file.delete())
        val request = Playback(fixture(), episode, listOf(episode), stream, 0.0, false, download)
        compose.activity.runOnUiThread {
            compose.activity.setContent {
                LevisioneTheme {
                    PlaybackScreen(request, offline, "en", false, {}, { _, _, _, _ -> }, {}, {})
                }
            }
        }
        startPlaybackChoosingEnglish()
        compose.onNodeWithTag("audioButton").performClick()
        compose.onAllNodesWithText("English", substring = true).onFirst().assertIsDisplayed()
        compose.onNodeWithTag("videoPlayer").assertExists()
        compose.activity.runOnUiThread { compose.activity.setContent { Text("Done") } }
        compose.runOnIdle { offline.remove(download.id) }
    }

    @Test
    fun hlsDownloadKeepsChosenLanguageAndPlaysWithoutItsSource() {
        val directory =
            compose.activity.cacheDir.resolve("playlist/hls-${UUID.randomUUID()}").apply {
                mkdirs()
            }
        val assets = InstrumentationRegistry.getInstrumentation().context.assets
        for (name in assets.list("hls").orEmpty()) assets.open("hls/$name").use { input ->
            directory.resolve(name).outputStream().use { input.copyTo(it) }
        }
        val episode = Episode("hls-${UUID.randomUUID()}", "1", "HLS offline regression")
        val metadata = DownloadMetadata(fixture(), episode, "", "en")
        val stream = StreamInfo(Uri.fromFile(directory.resolve("master.m3u8")).toString(), "")
        val offline = (compose.activity.application as AnimeApplication).offline
        val download = runBlocking { offline.prepare(metadata, stream, 480) }
        assertFalse(
            "HLS renditions must be selected, not downloaded wholesale",
            download.streamKeys.isEmpty(),
        )
        compose.runOnIdle { offline.add(download) }
        compose.waitUntil(30000) {
            offline.downloads.value.any {
                it.download.request.id == download.id &&
                    it.download.state == Download.STATE_COMPLETED
            }
        }
        assertTrue(directory.deleteRecursively())
        val request = Playback(fixture(), episode, listOf(episode), stream, 0.0, false, download)
        compose.activity.runOnUiThread {
            compose.activity.setContent {
                LevisioneTheme {
                    PlaybackScreen(request, offline, "en", false, {}, { _, _, _, _ -> }, {}, {})
                }
            }
        }
        startPlaybackChoosingEnglish()
        compose.onNodeWithTag("audioButton").performClick()
        compose.onAllNodesWithText("English", substring = true).onFirst().assertIsDisplayed()
        compose.activity.runOnUiThread { compose.activity.setContent { Text("Done") } }
        compose.runOnIdle { offline.remove(download.id) }
    }

    @Test
    fun videoFillsTheScreenAndFullscreenTogglesLandscape() {
        val file = copyFixture("fullscreen-fixture")
        val episode = Episode("fullscreen", "1", "Fullscreen regression")
        val request =
            Playback(
                fixture(),
                episode,
                listOf(episode),
                StreamInfo(Uri.fromFile(file).toString(), ""),
                0.0,
                false,
            )
        val offline = (compose.activity.application as AnimeApplication).offline
        compose.activity.runOnUiThread {
            compose.activity.setContent {
                LevisioneTheme {
                    PlaybackScreen(request, offline, "it", false, {}, { _, _, _, _ -> }, {}, {})
                }
            }
        }
        compose.waitUntil(20000) { englishDialogReady() }
        compose
            .onNodeWithTag("playbackStatus")
            .assertTextContains("Choose audio to start", substring = true)
        compose.onAllNodesWithText("English", substring = true).onFirst().performClick()
        waitForText("Playing")
        compose.onNodeWithTag("playPause").performClick()

        fun videoFills(fraction: Float): Boolean {
            val root = compose.onNodeWithTag("playerRoot").fetchSemanticsNode().boundsInRoot
            val video = compose.onNodeWithTag("videoPlayer").fetchSemanticsNode().boundsInRoot
            return video.width >= root.width * fraction && video.height >= root.height * fraction
        }

        compose.waitUntil(10000) { videoFills(0.95f) }
        assertEquals(
            ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED,
            compose.activity.requestedOrientation,
        )
        compose.onNodeWithTag("toggleFullscreen").performClick()
        compose.waitUntil(10000) {
            compose.activity.requestedOrientation ==
                ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        }
        compose.waitUntil(10000) { videoFills(0.95f) }
        compose.onNodeWithTag("toggleFullscreen").performClick()
        compose.waitUntil(10000) {
            compose.activity.requestedOrientation == ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        }
        compose.activity.runOnUiThread { compose.activity.setContent { Text("Done") } }
        file.delete()
    }

    @Test
    fun retryWithNewSignedUrlReplacesTheFailedRequest() {
        val file = copyFixture("retry-fixture")
        val episode = Episode("retry-${UUID.randomUUID()}", "1", "Retry regression")
        val metadata = DownloadMetadata(fixture(), episode, "", "en")
        val offline = (compose.activity.application as AnimeApplication).offline
        val oldUri =
            Uri.fromFile(compose.activity.cacheDir.resolve("missing-${UUID.randomUUID()}.mp4"))
        val failed = runBlocking {
            offline.prepare(metadata, StreamInfo(oldUri.toString(), ""), 480)
        }
        val retryCount = offline.manager.minRetryCount
        compose.runOnIdle {
            offline.manager.minRetryCount = 0
            offline.add(failed)
        }
        compose.waitUntil(30000) {
            offline.downloads.value.any {
                it.download.request.id == failed.id && it.download.state == Download.STATE_FAILED
            }
        }
        val fresh = runBlocking {
            offline.prepare(metadata, StreamInfo(Uri.fromFile(file).toString(), ""), 720)
        }
        compose.runOnIdle {
            offline.manager.minRetryCount = retryCount
            offline.add(fresh)
        }
        compose.waitUntil(30000) {
            offline.downloads.value.any {
                it.download.request.id == fresh.id &&
                    it.download.state == Download.STATE_COMPLETED &&
                    it.download.request.uri == fresh.uri
            }
        }
        compose.runOnIdle { offline.remove(fresh.id) }
        file.delete()
    }

    @Test
    fun liveSearchListsEpisodesAndResolvesStreamOverHttps() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("liveProviders") == "true")
        AnimeLib().use { lib ->
            val results = lib.search("frieren")
            assertFalse("at least one HTTPS provider must answer", results.isEmpty())
            val detail = lib.details(results.first())
            assertFalse(detail.episodes.isEmpty())
            val stream = lib.stream(detail.anime, detail.episodes.first())
            assertTrue(stream.url.startsWith("https://"))
            assertTrue(stream.referer.startsWith("https://"))
        }
    }

    @Test
    fun liveStreamingCommunityKeepsRefererAndTitleContext() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("liveProviders") == "true")
        AnimeLib().use { lib ->
            val anime =
                lib.search("one piece").first {
                    it.provider == ProviderId.STREAMING_COMMUNITY && it.id == "6654-one-piece"
                }
            assertTrue(anime.coverUrl.startsWith("https://"))
            val detail = lib.details(anime)
            assertFalse(detail.episodes.isEmpty())
            val stream = lib.stream(detail.anime, detail.episodes.first())
            assertTrue(stream.url.startsWith("https://"))
            assertTrue(stream.referer.contains("vixcloud"))
            val request =
                Playback(detail.anime, detail.episodes.first(), detail.episodes, stream, 0.0, false)
            val offline = (compose.activity.application as AnimeApplication).offline
            compose.activity.runOnUiThread {
                compose.activity.setContent {
                    LevisioneTheme {
                        PlaybackScreen(request, offline, "it", false, {}, { _, _, _, _ -> }, {}, {})
                    }
                }
            }
            startPlaybackChoosingEnglish()
            compose.activity.runOnUiThread { compose.activity.setContent { Text("Done") } }
        }
    }
}
