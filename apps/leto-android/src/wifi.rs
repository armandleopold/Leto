//! Android's `WifiManager.MulticastLock`. Without it the Wi-Fi chip drops
//! multicast packets to save power, and the answers PCs announce themselves
//! with over mDNS never reach the app.

use jni::{
    JavaVM,
    objects::{GlobalRef, JObject, JValue},
};

/// Held for as long as the app runs, given back when dropped.
pub struct MulticastLock {
    vm: JavaVM,
    lock: GlobalRef,
}

impl MulticastLock {
    /// Takes the lock for `activity`. The manifest asks for the
    /// `CHANGE_WIFI_MULTICAST_STATE` permission this needs.
    pub fn take(activity: &leto::AndroidApp) -> jni::errors::Result<Self> {
        // SAFETY: the activity gives the Java VM it runs in.
        let vm = unsafe { JavaVM::from_raw(activity.vm_as_ptr().cast())? };
        let lock = {
            let mut env = vm.attach_current_thread()?;
            // SAFETY: the activity object stays alive as long as the app.
            let context = unsafe { JObject::from_raw(activity.activity_as_ptr().cast()) };
            let name = env.new_string("wifi")?;
            let wifi = env
                .call_method(
                    &context,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&name)],
                )?
                .l()?;
            let tag = env.new_string("leto")?;
            let lock = env
                .call_method(
                    &wifi,
                    "createMulticastLock",
                    "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;",
                    &[JValue::Object(&tag)],
                )?
                .l()?;
            env.call_method(&lock, "setReferenceCounted", "(Z)V", &[JValue::Bool(0)])?;
            env.call_method(&lock, "acquire", "()V", &[])?;
            env.new_global_ref(lock)?
        };
        Ok(Self { vm, lock })
    }
}

impl Drop for MulticastLock {
    fn drop(&mut self) {
        if let Ok(mut env) = self.vm.attach_current_thread() {
            let _ = env.call_method(&self.lock, "release", "()V", &[]);
        }
    }
}
