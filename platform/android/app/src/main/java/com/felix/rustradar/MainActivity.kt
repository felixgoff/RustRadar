package com.felix.rustradar

import android.os.Bundle
import android.view.KeyEvent
import android.view.View
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity
import com.google.androidgamesdk.gametextinput.State

/**
 * The native app's activity (doc 07 §13.4). GameActivity loads librustradar_app.so (the
 * manifest's `android.app.lib_name`), runs its `android_main` on a thread of its own and keeps the
 * soft keyboard's text in GameTextInput. This class adds what GameActivity does not hand to Rust,
 * for `crates/app/src/platform/android.rs`: the window insets, the back gesture, the keyboard's
 * action key and a nudge when the keyboard changed the text.
 */
class MainActivity : GameActivity() {
    // Always on: Rust decides whether back closes something, and calls leave() when not. Before
    // the app listens (nativeBack answers false) back leaves at once.
    private val back = object : OnBackPressedCallback(true) {
        override fun handleOnBackPressed() {
            if (!nativeBack()) leave()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // the map draws under the system bars; the UI pads by the insets (as the Tauri build did)
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        onBackPressedDispatcher.addCallback(this, back)
    }

    // GameActivity hands every key to Rust, which would swallow back. The back key goes to the
    // dispatcher instead, like the gesture (which Android 13+ sends there itself).
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_BACK) {
            if (event.action == KeyEvent.ACTION_UP && !event.isCanceled) {
                onBackPressedDispatcher.onBackPressed()
            }
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    // The insets GameActivity's surface view gets: what the system bars and the cutout cover, and
    // the keyboard's height, device pixels.
    override fun onApplyWindowInsets(v: View, insets: WindowInsetsCompat): WindowInsetsCompat {
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val keyboard = insets.getInsets(WindowInsetsCompat.Type.ime())
        nativeInsets(bars.left, bars.top, bars.right, bars.bottom, keyboard.bottom)
        return super.onApplyWindowInsets(v, insets)
    }

    // The keyboard's Done key: android-activity gives it only to winit, which drops it.
    override fun onEditorAction(action: Int) {
        super.onEditorAction(action)
        nativeEditorAction(action)
    }

    // The keyboard changed the text: wake the app so a frame reads it.
    override fun stateChanged(newState: State, dismissed: Boolean) {
        super.stateChanged(newState, dismissed)
        nativeTextChanged()
    }

    /** Back that the app did not take: what back does without it (Rust calls this too). */
    fun leave() {
        runOnUiThread {
            back.isEnabled = false
            onBackPressedDispatcher.onBackPressed()
            back.isEnabled = true
        }
    }

    private external fun nativeInsets(left: Int, top: Int, right: Int, bottom: Int, keyboard: Int)
    private external fun nativeBack(): Boolean
    private external fun nativeEditorAction(action: Int)
    private external fun nativeTextChanged()
}
