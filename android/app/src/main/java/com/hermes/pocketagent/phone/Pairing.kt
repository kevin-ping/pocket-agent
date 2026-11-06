package com.hermes.pocketagent.phone

import android.net.Uri
import android.util.Base64
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.net.Inet4Address
import java.net.NetworkInterface
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * What a PA's pairing QR code says (src-tauri/src/api/pairing.rs):
 * `pocketagent://pair?v=1&id=<pa_id>&s=<secret>&h=<ip:port>,…`.
 * The code is untrusted input: anything off-format is refused before any request.
 */
class PairQr(val paId: String, val secret: ByteArray, val hosts: List<String>) {
    companion object {
        private const val MAX_TEXT = 1024
        private const val MAX_HOSTS = 16
        private val ID = Regex("[A-Za-z0-9_-]{1,64}")
        private val SECRET = Regex("[A-Za-z0-9_-]{43}")

        fun parse(text: String): PairQr? {
            if (text.length > MAX_TEXT) return null
            val uri = try { Uri.parse(text) } catch (e: Exception) { return null }
            if (uri.scheme != "pocketagent" || uri.host != "pair" || uri.getQueryParameter("v") != "1") return null
            val id = uri.getQueryParameter("id")?.takeIf { ID.matches(it) } ?: return null
            val s = uri.getQueryParameter("s")?.takeIf { SECRET.matches(it) } ?: return null
            val secret = try { Base64.decode(s, B64) } catch (e: IllegalArgumentException) { return null }
            if (secret.size != 32) return null
            val entries = uri.getQueryParameter("h")?.split(',').orEmpty()
            if (entries.isEmpty() || entries.size > MAX_HOSTS) return null
            val hosts = entries.map { entry ->
                val host = entry.takeIf { it.contains(':') }?.let { Pa.parseHost(it) } ?: return null
                if (!privateIpv4(host.substringBeforeLast(':'))) return null
                host
            }
            return PairQr(id, secret, hosts.distinct())
        }

        /** 10/8, 172.16/12, 192.168/16 written as four plain decimal octets. */
        fun privateIpv4(ip: String): Boolean {
            val parts = ip.split('.')
            if (parts.size != 4) return false
            val b = parts.map { p -> p.toIntOrNull()?.takeIf { it in 0..255 && p == it.toString() } ?: return false }
            return b[0] == 10 || (b[0] == 172 && b[1] in 16..31) || (b[0] == 192 && b[1] == 168)
        }
    }
}

private const val B64 = Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP

sealed class PairResult {
    class Paired(val paId: String, val name: String, val host: String, val token: String) : PairResult()
    /** No QR code on that computer any more: expired, used, cancelled, or replaced. */
    object Expired : PairResult()
    object Unreachable : PairResult()
    /** The reply did not prove it holds the secret: not that computer. Nothing saved. */
    object NotThatComputer : PairResult()
    class Failed(val status: Int) : PairResult()
}

/**
 * The phone's side of the protocol (FEAT-QR2 / SEC-QR2). Blocking: call off the
 * main thread. Neither the secret, the proofs nor the token are ever logged.
 */
object Pairing {
    private val http = OkHttpClient.Builder().callTimeout(4, TimeUnit.SECONDS).build()

    private fun hmac(secret: ByteArray, side: String, nonce: String, phoneId: String): ByteArray =
        Mac.getInstance("HmacSHA256").run {
            init(SecretKeySpec(secret, "HmacSHA256"))
            doFinal("$side|$nonce|$phoneId".toByteArray(Charsets.UTF_8))
        }

    /** This phone's own IPv4 addresses (no permission needed). */
    private fun ownIpv4s(): List<String> = try {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && !it.isLoopback }
            .flatMap { it.inetAddresses.toList() }
            .filterIsInstance<Inet4Address>()
            .mapNotNull { it.hostAddress }
    } catch (e: Exception) {
        emptyList()
    }

    /** Addresses on this phone's /24 first, the rest in the code's order. */
    fun ordered(hosts: List<String>, own: List<String> = ownIpv4s()): List<String> {
        val nets = own.map { it.substringBeforeLast('.') }.toSet()
        return hosts.sortedBy { if (it.substringBeforeLast(':').substringBeforeLast('.') in nets) 0 else 1 }
    }

    private fun call(request: Request): Pair<Int, JSONObject?> = try {
        http.newCall(request).execute().use { r ->
            r.code to (try { JSONObject(r.body?.string().orEmpty()) } catch (e: Exception) { null })
        }
    } catch (e: Exception) {
        0 to null
    }

    fun pair(qr: PairQr, phoneId: String, phoneName: String): PairResult {
        var expired = false
        for (host in ordered(qr.hosts)) {
            val (status, hello) = call(Request.Builder().url("http://$host/pair/hello").get().build())
            if (status == 410) expired = true
            // Only the PA the code names gets a proof.
            if (status != 200 || hello == null || hello.optString("pa_id") != qr.paId) continue
            val nonce = hello.optString("nonce").takeIf { it.isNotEmpty() } ?: continue
            return confirm(qr, host, nonce, hello.optString("pa_name"), phoneId, phoneName)
        }
        return if (expired) PairResult.Expired else PairResult.Unreachable
    }

    private fun confirm(qr: PairQr, host: String, nonce: String, helloName: String, phoneId: String, phoneName: String): PairResult {
        val body = JSONObject()
            .put("phone_id", phoneId).put("phone_name", phoneName).put("nonce", nonce)
            .put("proof", Base64.encodeToString(hmac(qr.secret, "phone", nonce, phoneId), B64))
        val request = Request.Builder().url("http://$host/pair/confirm")
            .post(body.toString().toRequestBody("application/json".toMediaType())).build()
        val (status, reply) = call(request)
        return when {
            status == 200 && reply != null -> {
                val claimed = try { Base64.decode(reply.optString("server_proof"), B64) } catch (e: IllegalArgumentException) { ByteArray(0) }
                val token = reply.optString("token")
                if (!MessageDigest.isEqual(claimed, hmac(qr.secret, "pa", nonce, phoneId)) || token.isEmpty()) {
                    PairResult.NotThatComputer
                } else {
                    PairResult.Paired(qr.paId, reply.optString("pa_name").ifEmpty { helloName }, host, token)
                }
            }
            status == 410 || status == 403 -> PairResult.Expired
            status == 0 -> PairResult.Unreachable
            else -> PairResult.Failed(status)
        }
    }
}
