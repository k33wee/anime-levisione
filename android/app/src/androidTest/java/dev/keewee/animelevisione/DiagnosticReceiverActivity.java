package dev.keewee.animelevisione;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.os.Process;
import java.io.InputStream;
import java.io.OutputStream;

/** Separate test-APK UID; Java avoids relying on the target APK's Kotlin runtime. */
public class DiagnosticReceiverActivity extends Activity {
    @Override
    @SuppressWarnings("deprecation")
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Uri uri = getIntent().getParcelableExtra(Intent.EXTRA_STREAM);
        boolean canRead = false;
        boolean canWrite = false;
        try (InputStream input = getContentResolver().openInputStream(uri)) {
            canRead = input != null && input.read() >= 0;
        } catch (Exception ignored) { }
        try (OutputStream output = getContentResolver().openOutputStream(uri, "wa")) {
            canWrite = output != null;
        } catch (Exception ignored) { }
        Intent result = new Intent(getIntent().getStringExtra("resultAction"));
        result.setPackage("dev.keewee.animelevisione");
        result.putExtra("canRead", canRead);
        result.putExtra("canWrite", canWrite);
        result.putExtra("uid", Process.myUid());
        sendBroadcast(result);
        finish();
    }
}
