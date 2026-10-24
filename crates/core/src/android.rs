//! Android JVM/context setup, called by AnimeApplication before any network work.

use jni::{EnvUnowned, errors::ThrowRuntimeExAndDefault, jni_mangle, objects::JObject};

#[jni_mangle("dev.keewee.animelevisione.AnimeApplication")]
pub fn initialize<'caller>(
    mut env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    context: JObject<'caller>,
) {
    // The verifier retains global references for the lifetime of the process.
    // with_env catches panics; resolve reports initialization errors to Kotlin.
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>();
}
