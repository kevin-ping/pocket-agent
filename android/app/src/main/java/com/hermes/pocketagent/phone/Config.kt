package com.hermes.pocketagent.phone

import android.content.Context
import android.os.Build
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import org.json.JSONArray
import org.json.JSONObject
import java.net.URLEncoder
import java.util.UUID

/**
 * One computer running PA. [token] is the device token from pairing, or the
 * API_SERVER_KEY for a computer entered by hand. [key] is local and never
 * changes; [paId] is PA's own id (empty for a hand-entered one until its
 * `/phone` welcome tells us).
 */
data class Pa(val key: String, val paId: String, val name: String, val host: String, val token: String) {
    private val hostPort get() = if (host.contains(':')) host else "$host:8650"
    private val encodedToken get() = URLEncoder.encode(token, "UTF-8")

    fun uiUrl() = "${uiPrefix()}?token=$encodedToken"
    /** Pages under this prefix (PA's own frontend) may use the camera bridge. */
    fun uiPrefix() = "http://$hostPort/ui/"
    fun audioUrl() = "ws://$hostPort/audio?token=$encodedToken"
    fun phoneUrl() = "ws://$hostPort/phone?token=$encodedToken"

    fun toJson(): JSONObject = JSONObject()
        .put("key", key).put("pa_id", paId).put("name", name).put("host", host).put("token", token)

    companion object {
        fun fromJson(o: JSONObject) = Pa(
            o.getString("key"), o.optString("pa_id"), o.optString("name"), o.getString("host"), o.getString("token"),
        )

        fun hostPort(host: String) = if (host.contains(':')) host else "$host:8650"

        /** A typed address as `host:port` (port 8650 implied), or null when it is not one. */
        fun parseHost(input: String): String? {
            val url = "http://${hostPort(input.trim())}/".toHttpUrlOrNull() ?: return null
            if (url.encodedPath != "/" || url.query != null || url.username.isNotEmpty()) return null
            val host = if (url.host.contains(':')) "[${url.host}]" else url.host
            return "$host:${url.port}"
        }
    }
}

/**
 * The paired PAs, which one uses the phone (the occupant: the phone alone
 * decides, see AudioService), and this phone's own id.
 */
object Config {
    private const val PREFS = "pocket-agent"

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Before multiple PAs: one hand-entered host + key becomes a hand-entered entry. */
    private fun migrate(context: Context) {
        val p = prefs(context)
        if (p.contains("pas")) return
        val host = p.getString("host", "").orEmpty().trim()
        val key = p.getString("key", "").orEmpty().trim()
        val list = JSONArray()
        if (host.isNotEmpty() && key.isNotEmpty()) list.put(Pa(newKey(), "", host, host, key).toJson())
        p.edit().putString("pas", list.toString()).remove("host").remove("key").apply()
    }

    private fun newKey() = UUID.randomUUID().toString()

    fun pas(context: Context): List<Pa> {
        migrate(context)
        val array = JSONArray(prefs(context).getString("pas", "[]"))
        return (0 until array.length()).map { Pa.fromJson(array.getJSONObject(it)) }
    }

    private fun savePas(context: Context, list: List<Pa>) {
        prefs(context).edit().putString("pas", JSONArray(list.map { it.toJson() }).toString()).apply()
    }

    fun pa(context: Context, key: String?) = key?.let { k -> pas(context).firstOrNull { it.key == k } }

    /** A new pairing. Pairing the same PA again (by pa_id) replaces it, keeping its local key. */
    fun add(context: Context, paId: String, name: String, host: String, token: String): Pa {
        val list = pas(context)
        val old = list.firstOrNull { paId.isNotEmpty() && it.paId == paId }
        val pa = Pa(old?.key ?: newKey(), paId, name, host, token)
        savePas(context, list.filter { it.key != pa.key } + pa)
        return pa
    }

    fun update(context: Context, pa: Pa) {
        savePas(context, pas(context).map { if (it.key == pa.key) pa else it })
    }

    fun remove(context: Context, key: String) {
        savePas(context, pas(context).filter { it.key != key })
        if (occupant(context) == key) setOccupant(context, null)
    }

    /** The local key of the PA using the phone, or null. */
    fun occupant(context: Context): String? = prefs(context).getString("occupant", null)

    fun setOccupant(context: Context, key: String?) {
        prefs(context).edit().apply { if (key == null) remove("occupant") else putString("occupant", key) }.apply()
    }

    /** Stable for this install; PA keys the pairing on it. */
    fun phoneId(context: Context): String {
        val p = prefs(context)
        return p.getString("phone_id", null) ?: newKey().also { p.edit().putString("phone_id", it).apply() }
    }

    fun phoneName(): String = Build.MODEL.ifBlank { "Android" }
}
