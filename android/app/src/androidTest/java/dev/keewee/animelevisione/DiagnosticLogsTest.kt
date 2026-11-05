package dev.keewee.animelevisione

import android.content.Intent
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.IntentFilter
import android.os.Process
import androidx.core.content.ContextCompat
import android.net.Uri
import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.IOException
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.anime_levisione_core.*

@RunWith(AndroidJUnit4::class)
class DiagnosticLogsTest {
    @Test
    fun anotherAppNeedsAnExplicitReadGrantAndNeverGetsWriteAccess() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        for (grant in listOf(false, true)) {
            val action = "dev.keewee.animelevisione.DIAGNOSTICS_TEST_${UUID.randomUUID()}"
            val latch = CountDownLatch(1)
            var result: Intent? = null
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    result = intent
                    latch.countDown()
                }
            }
            ContextCompat.registerReceiver(context, receiver, IntentFilter(action), ContextCompat.RECEIVER_EXPORTED)
            try {
                val intent = DiagnosticLogs.shareIntent(context).apply {
                    component = ComponentName(instrumentation.context.packageName, DiagnosticReceiverActivity::class.java.name)
                    putExtra("resultAction", action)
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    if (!grant) {
                        // ACTION_SEND migrates EXTRA_STREAM into ClipData and adds a
                        // read grant in Android itself. Use a custom action to test denial.
                        this.action = "dev.keewee.animelevisione.TEST_NO_GRANT"
                        flags = Intent.FLAG_ACTIVITY_NEW_TASK
                        clipData = null
                    }
                }
                context.startActivity(intent)
                assertTrue("Receiver app must respond", latch.await(15, TimeUnit.SECONDS))
                assertNotEquals(Process.myUid(), result!!.getIntExtra("uid", -1))
                assertEquals(grant, result!!.getBooleanExtra("canRead", false))
                assertFalse(result!!.getBooleanExtra("canWrite", false))
            } finally {
                context.unregisterReceiver(receiver)
            }
        }
    }

    @Test
    fun exportsOnlyBoundedRedactedSnapshotsWithReadOnlyContentGrants() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertTrue(DiagnosticLogs.available)
        val secret = "SECRET-alib1.bot-token-title-https://private.example/path?token=abc"
        val unrelated = context.filesDir.resolve("sensitive-test-data.json").apply { writeText(secret) }
        try {
            val crash = IllegalStateException(secret, IOException(secret)).apply {
                stackTrace = arrayOf(StackTraceElement("dev.keewee.animelevisione.CatalogViewModel", secret, "CatalogViewModel.kt", 123))
            }
            DiagnosticLogs.recordCrash(crash)
            DiagnosticLogs.record(DiagnosticEvent.PlayerFailure(99u, 2004u, DiagnosticFailure.HTTP))
            val intent = DiagnosticLogs.shareIntent(context)
            assertEquals(Intent.ACTION_SEND, intent.action)
            assertEquals("text/plain", intent.type)
            assertNotEquals(0, intent.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION)
            assertEquals(0, intent.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            @Suppress("DEPRECATION")
            val uri = intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)!!
            assertEquals("content", uri.scheme)
            assertEquals("${context.packageName}.diagnostics", uri.authority)
            assertEquals(uri, intent.clipData!!.getItemAt(0).uri)
            val text = context.contentResolver.openInputStream(uri)!!.bufferedReader().use { it.readText() }
            assertTrue(text.toByteArray().size <= 1024 * 1024)
            assertTrue(text.contains("android_crash"))
            assertTrue(text.contains("android_crash_frame"))
            assertTrue(text.contains("\"line\":123"))
            assertTrue(text.contains("\"code\":2004"))
            for (fragment in listOf(secret, "private.example", "bot-token", "https://", context.filesDir.absolutePath)) {
                assertFalse("Sensitive text must not enter diagnostics: $fragment", text.contains(fragment))
            }
            text.lineSequence().filter { it.isNotBlank() }.forEach { JSONObject(it) }
            val provider = context.packageManager.resolveContentProvider(uri.authority!!, 0)!!
            assertFalse(provider.exported)
            assertTrue(provider.grantUriPermissions)
            try {
                FileProvider.getUriForFile(context, uri.authority!!, unrelated)
                fail("The diagnostic provider must not expose app data")
            } catch (_: IllegalArgumentException) { }
            repeat(4) { DiagnosticLogs.shareIntent(context) }
            assertTrue(context.cacheDir.resolve("diagnostic-exports").listFiles()!!.size <= 3)
        } finally {
            unrelated.delete()
        }
    }
}
