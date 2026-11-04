package com.hermes.pocketagent.phone

import android.content.Context
import java.net.URLEncoder

/** Mac address + API_SERVER_KEY, entered once on first launch. */
data class Config(val host: String, val key: String) {
    private val hostPort get() = if (host.contains(':')) host else "$host:8650"
    private val token get() = URLEncoder.encode(key, "UTF-8")

    fun uiUrl() = "${uiPrefix()}?token=$token"
    /** Pages under this prefix (PA's own frontend) may use the camera bridge. */
    fun uiPrefix() = "http://$hostPort/ui/"
    fun audioUrl() = "ws://$hostPort/audio?token=$token"

    companion object {
        private const val PREFS = "pocket-agent"

        fun load(context: Context): Config? {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val host = prefs.getString("host", "").orEmpty().trim()
            val key = prefs.getString("key", "").orEmpty().trim()
            return if (host.isEmpty() || key.isEmpty()) null else Config(host, key)
        }

        fun save(context: Context, config: Config) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString("host", config.host.trim())
                .putString("key", config.key.trim())
                .apply()
        }
    }
}
