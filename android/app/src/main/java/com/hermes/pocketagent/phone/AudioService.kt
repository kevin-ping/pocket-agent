package com.hermes.pocketagent.phone

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.NoiseSuppressor
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.json.JSONObject
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The phone's connections to its paired PAs, as a foreground service so they
 * survive the screen off and Home (a hidden WebView page is frozen).
 *
 * `/phone` — one per paired PA, always (see src-tauri/src/api/phone.rs): hello /
 * state tell every PA who uses the phone (the occupant) and whether the app is
 * on screen; a PA asks with `enter_request` and lets go with `release`. The
 * phone alone decides: while one PA occupies it every other is refused (busy).
 * Losing the occupant for [LEASE_MS] (its computer asleep or gone) frees the phone.
 *
 * `/audio` — to the occupant only, while there is one (see api/remote.rs):
 *  - up:   PCM16LE 16 kHz mono, 20 ms frames, from the VOICE_COMMUNICATION
 *          source so the platform echo canceller removes PA's own voice and
 *          barge-in keeps working;
 *  - down: `{"type":"play","seq":N}` + one binary mp3 → played in order,
 *          answered with `playing` / `done`; `{"type":"clear"}` stops at once;
 *          `{"type":"snapshot","id":N}` → one camera frame from [CameraHub];
 *          `{"type":"wake"}` → the owner said the wake word: light the screen
 *          and bring the page forward (a hidden page is frozen, see [MainActivity.voiceActive]);
 *  - up:   [CameraHub.stateJson] whenever the camera opens, closes or switches lens.
 * Swiping the app away or the notification's Disconnect frees the phone and stops
 * the service, so PA's voice returns to its computer.
 */
class AudioService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private val client = OkHttpClient.Builder()
        .pingInterval(10, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()

    @Volatile private var running = false

    // ── /phone links (main thread only) ─────────────────────────────────────

    private inner class Link(var pa: Pa) {
        var socket: WebSocket? = null
        var online = false
        /** When it was last lost (or the service started without it), for the lease. */
        var lostAt = SystemClock.elapsedRealtime()
    }

    private val links = linkedMapOf<String, Link>() // PA key → link

    // ── /audio (the occupant) ───────────────────────────────────────────────

    // Bumped on every start / stop: callbacks of an older /audio socket are ignored.
    @Volatile private var audioGen = 0
    private var audioPa: Pa? = null // main thread
    @Volatile private var socket: WebSocket? = null
    private var expectSeq: Long? = null // set by a play header, consumed by the next binary frame

    /** A sentence remembers the generation it arrived in; `clear` bumps the generation. */
    private class Sentence(val seq: Long, val mp3: ByteArray, val generation: Int)

    private val playQueue = LinkedBlockingQueue<Sentence>()
    // `clear` and the player's check-then-start share this lock, so once clear
    // returns no sentence from before it can start, and the one playing is stopped.
    private val playLock = Any()
    private var playGeneration = 0 // guarded by playLock
    private var playing: Pair<MediaPlayer, CountDownLatch>? = null // guarded by playLock

    private lateinit var audioManager: AudioManager
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var recorder: Thread? = null
    private var player: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_DISCONNECT) {
            Log.i(TAG, "disconnect from the notification")
            onDisconnect?.invoke() // MainActivity: close the camera and the page (/ui)
            leave()
            stopSelf()
            return START_NOT_STICKY
        }
        if (!running) {
            running = true
            instance = this
            startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            audioManager = getSystemService(AudioManager::class.java)
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "PocketAgent:audio").apply { acquire() }
            @Suppress("DEPRECATION")
            wifiLock = getSystemService(WifiManager::class.java)
                .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "PocketAgent:audio").apply { acquire() }
            CameraHub.listener = { state -> send(state) }
        }
        refresh()
        return START_STICKY
    }

    // Swiped away from recents. Home and screen lock don't come here. The
    // activity's onStop has already closed the camera.
    override fun onTaskRemoved(rootIntent: Intent?) {
        Log.i(TAG, "task removed: stopping")
        leave()
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        running = false
        if (instance === this) instance = null
        CameraHub.listener = null
        handler.removeCallbacksAndMessages(null)
        links.values.forEach { it.socket?.close(1000, "service stopped") }
        links.clear()
        stopAudio()
        wakeLock?.release()
        wifiLock?.release()
        super.onDestroy()
    }

    private fun notification(): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, getString(R.string.notification_channel), NotificationManager.IMPORTANCE_LOW)
        )
        val disconnect = PendingIntent.getService(
            this, 0,
            Intent(this, AudioService::class.java).setAction(ACTION_DISCONNECT),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val occupant = Config.pa(this, Config.occupant(this))
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Pocket Agent")
            .setContentText(if (occupant != null) getString(R.string.notification_using, occupant.name) else getString(R.string.notification_idle))
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, getString(R.string.notification_disconnect), disconnect).build())
            .build()
    }

    private fun updateNotification() {
        if (running) getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification())
    }

    // ── Paired PAs ──────────────────────────────────────────────────────────

    /** Re-reads the paired list: links follow it, /audio follows the occupant. */
    private fun refresh() {
        val pas = Config.pas(this).associateBy { it.key }
        for ((key, link) in links.entries.toList()) {
            val pa = pas[key]
            if (pa == null || pa.phoneUrl() != link.pa.phoneUrl()) {
                links.remove(key)
                link.socket?.close(1000, "unpaired")
            }
        }
        for (pa in pas.values) {
            val link = links[pa.key]
            if (link == null) Link(pa).also { links[pa.key] = it; connectLink(it) } else link.pa = pa
        }
        val occupant = Config.occupant(this)
        if (occupant != null && occupant !in pas) Config.setOccupant(this, null)
        occupant?.let { links[it] }?.let { if (!it.online) scheduleLease(it) }
        syncAudio()
        updateNotification()
        changed()
    }

    private fun connectLink(link: Link) {
        if (!running || links[link.pa.key] !== link) return
        val request = Request.Builder().url(link.pa.phoneUrl()).build()
        client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) = onMain {
                if (links[link.pa.key] !== link) return@onMain webSocket.cancel()
                link.socket = webSocket
                link.online = true
                webSocket.send(stateJson("hello").put("phone_id", Config.phoneId(this@AudioService))
                    .put("phone_name", Config.phoneName()).toString())
                changed()
            }

            override fun onMessage(webSocket: WebSocket, text: String) = onMain {
                if (link.socket === webSocket) onPaMessage(link, webSocket, text)
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lostLink(link, webSocket, reason)

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
                lostLink(link, webSocket, t.message ?: "failure")
        })
    }

    private fun lostLink(link: Link, webSocket: WebSocket, why: String) = onMain {
        if (link.socket === webSocket || link.socket == null) {
            if (link.online) Log.w(TAG, "/phone ${link.pa.name} lost: $why")
            link.socket = null
            if (link.online) link.lostAt = SystemClock.elapsedRealtime()
            link.online = false
            if (Config.occupant(this) == link.pa.key) scheduleLease(link)
            changed()
        }
        if (running && links[link.pa.key] === link) handler.postDelayed({ connectLink(link) }, RECONNECT_MS)
    }

    /** The occupant gone for LEASE_MS frees the phone; Wi-Fi blips don't. */
    private fun scheduleLease(link: Link) {
        val due = link.lostAt + LEASE_MS - SystemClock.elapsedRealtime()
        handler.postDelayed({
            val current = links[link.pa.key]
            if (current === link && !link.online && Config.occupant(this) == link.pa.key &&
                SystemClock.elapsedRealtime() - link.lostAt >= LEASE_MS
            ) {
                Log.i(TAG, "${link.pa.name} offline for ${LEASE_MS / 1000} s: phone is free")
                leave()
            }
        }, maxOf(due, 0L) + 50)
    }

    private fun onPaMessage(link: Link, webSocket: WebSocket, text: String) {
        val msg = try { JSONObject(text) } catch (e: Exception) { return }
        when (msg.optString("type")) {
            "welcome" -> {
                val paId = msg.optString("pa_id")
                val name = msg.optString("pa_name").ifEmpty { link.pa.name }
                if (paId != link.pa.paId || name != link.pa.name) {
                    link.pa = link.pa.copy(paId = paId, name = name)
                    Config.update(this, link.pa)
                    // A hand-entered PA just learnt its id: tell it whether it is the occupant.
                    webSocket.send(stateJson("state").toString())
                    updateNotification()
                    changed()
                }
            }
            "enter_request" -> {
                val reply = JSONObject().put("type", "enter_reply").put("req_id", msg.optLong("req_id"))
                val occupant = Config.occupant(this)
                when {
                    occupant != null && occupant != link.pa.key -> reply.put("ok", false).put("reason", "busy")
                        .put("busy_with", Config.pa(this, occupant)?.name.orEmpty())
                    !foreground -> reply.put("ok", false).put("reason", "not_foreground")
                    else -> reply.put("ok", true)
                }
                webSocket.send(reply.toString())
                if (reply.getBoolean("ok")) enter(link.pa.key)
            }
            "release" -> if (Config.occupant(this) == link.pa.key) {
                Log.i(TAG, "${link.pa.name} moved back to its computer")
                leave()
            }
        }
    }

    private fun stateJson(type: String): JSONObject {
        val occupant = Config.pa(this, Config.occupant(this))
        return JSONObject().put("type", type).put("foreground", foreground).put(
            "occupant",
            occupant?.let { JSONObject().put("pa_id", it.paId).put("pa_name", it.name) } ?: JSONObject.NULL,
        )
    }

    private fun broadcastState() {
        val state = stateJson("state").toString()
        links.values.forEach { it.socket?.send(state) }
    }

    /** Use PA [key]. Refused (false) while another PA occupies the phone or [key] is offline. */
    private fun enter(key: String): Boolean {
        val occupant = Config.occupant(this)
        if (occupant != null && occupant != key) return false
        if (occupant == null && links[key]?.online != true) return false
        if (occupant == null) {
            Config.setOccupant(this, key)
            Log.i(TAG, "now used by ${Config.pa(this, key)?.name}")
            broadcastState()
            syncAudio()
            updateNotification()
        }
        onOccupant?.invoke(key)
        changed()
        return true
    }

    /** Free the phone: /audio and the mic stop, every PA hears the occupant is gone. */
    private fun leave() {
        if (Config.occupant(this) == null) return
        Config.setOccupant(this, null)
        broadcastState()
        syncAudio()
        updateNotification()
        onOccupant?.invoke(null)
        changed()
    }

    private fun changed() = onChanged?.invoke()

    private fun onMain(action: () -> Unit) {
        handler.post(action)
    }

    // ── /audio ──────────────────────────────────────────────────────────────

    private fun syncAudio() {
        val pa = Config.pa(this, Config.occupant(this))
        if (pa?.audioUrl() == audioPa?.audioUrl()) return
        stopAudio()
        if (pa != null) startAudio(pa)
    }

    private fun startAudio(pa: Pa) {
        audioPa = pa
        val gen = ++audioGen
        audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
        @Suppress("DEPRECATION") // setCommunicationDevice is API 31+; Pixel 2 is API 30
        audioManager.isSpeakerphoneOn = true
        recorder = Thread({ recordLoop(gen) }, "pa-recorder").also { it.start() }
        player = Thread({ playLoop(gen) }, "pa-player").also { it.start() }
        connect(pa, gen)
    }

    private fun stopAudio() {
        if (audioPa == null) return
        audioPa = null
        audioGen++
        socket?.close(1000, "released")
        socket = null
        clearPlayback()
        player?.interrupt()
        recorder?.join(500)
        player = null
        recorder = null
        if (::audioManager.isInitialized) audioManager.mode = AudioManager.MODE_NORMAL
    }

    private fun connect(pa: Pa, gen: Int) {
        if (!running || gen != audioGen) return
        client.newWebSocket(Request.Builder().url(pa.audioUrl()).build(), object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                if (gen != audioGen) {
                    webSocket.close(1000, "released")
                    return
                }
                Log.i(TAG, "audio connected to ${pa.name}")
                socket = webSocket
                webSocket.send(CameraHub.stateJson()) // PA starts every phone with the camera closed
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                if (gen != audioGen) return
                val msg = try { JSONObject(text) } catch (e: Exception) { return }
                when (msg.optString("type")) {
                    "play" -> {
                        expectSeq = msg.getLong("seq")
                        voiceActive(wake = false)
                    }
                    "wake" -> voiceActive(wake = true)
                    "clear" -> clearPlayback()
                    "snapshot" -> {
                        val id = msg.getLong("id")
                        // Answer on the socket that asked; a replaced one is ignored by PA anyway.
                        CameraHub.snapshot { jpeg ->
                            val reply = JSONObject().put("type", "snapshot").put("id", id)
                            if (jpeg != null) reply.put("jpeg", jpeg)
                            webSocket.send(reply.toString())
                        }
                    }
                }
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                if (gen != audioGen) return
                val seq = expectSeq ?: return
                expectSeq = null
                synchronized(playLock) { playQueue.put(Sentence(seq, bytes.toByteArray(), playGeneration)) }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lost(webSocket, pa, gen, reason)

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
                lost(webSocket, pa, gen, t.message ?: "failure")
        })
    }

    private fun lost(webSocket: WebSocket, pa: Pa, gen: Int, why: String) {
        if (gen != audioGen) return
        Log.w(TAG, "audio connection lost: $why")
        if (socket === webSocket) socket = null
        // PA has switched back to its computer's own speaker; nothing queued here still applies.
        clearPlayback()
        if (running) handler.postDelayed({ connect(pa, gen) }, RECONNECT_MS)
    }

    /** On the main thread, like the activity's other callbacks. */
    private fun voiceActive(wake: Boolean) = handler.post {
        val activity = onVoiceActive
        if (activity != null) activity(wake)
        else if (wake) MainActivity.wake(this) // the system destroyed the activity meanwhile
    }

    private fun send(json: String) {
        socket?.send(json)
    }

    // ── Microphone ──────────────────────────────────────────────────────────

    @SuppressLint("MissingPermission") // MainActivity starts the service only after RECORD_AUDIO is granted
    /** Until /audio stops or moves to another PA ([gen] no longer current). */
    private fun recordLoop(gen: Int) {
        val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record = AudioRecord(
            MediaRecorder.AudioSource.VOICE_COMMUNICATION,
            RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
            maxOf(min, FRAME_BYTES * 4),
        )
        if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(record.audioSessionId)?.enabled = true
        if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(record.audioSessionId)?.enabled = true
        val frame = ByteArray(FRAME_BYTES)
        record.startRecording()
        try {
            while (running && gen == audioGen) {
                val n = record.read(frame, 0, frame.size)
                // Stream continuously; PA decides when it is listening.
                if (n > 0) socket?.send(frame.toByteString(0, n))
            }
        } finally {
            record.stop()
            record.release()
        }
    }

    // ── Speaker ─────────────────────────────────────────────────────────────

    private fun playLoop(gen: Int) {
        while (running && gen == audioGen) {
            val sentence = try { playQueue.take() } catch (e: InterruptedException) { return }
            play(sentence)
        }
    }

    private fun isCurrent(sentence: Sentence) = synchronized(playLock) { sentence.generation == playGeneration }

    /** Plays one sentence on this thread; `clear` stops it and releases the latch early. */
    private fun play(sentence: Sentence) {
        val seq = sentence.seq
        if (!isCurrent(sentence)) return
        val file = File(cacheDir, "sentence-$seq.mp3").apply { writeBytes(sentence.mp3) }
        val done = CountDownLatch(1)
        val mp = MediaPlayer()
        try {
            mp.setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build()
            )
            mp.setDataSource(file.path)
            mp.setOnCompletionListener { done.countDown() }
            mp.setOnErrorListener { _, _, _ -> done.countDown(); true }
            mp.prepare()
            val started = synchronized(playLock) {
                if (sentence.generation != playGeneration) return@synchronized false
                playing = mp to done
                mp.start()
                true
            }
            if (!started) return
            send("""{"type":"playing","seq":$seq}""")
            done.await(PLAY_TIMEOUT_S, TimeUnit.SECONDS)
        } catch (e: Exception) {
            Log.w(TAG, "sentence $seq failed: ${e.message}")
        } finally {
            synchronized(playLock) {
                if (playing?.first === mp) playing = null
                try { if (mp.isPlaying) mp.stop() } catch (e: IllegalStateException) { /* never prepared */ }
            }
            mp.release()
            file.delete()
        }
        // A cleared sentence needs no report: PA already moved on. A failed one
        // reports done without playing, so PA doesn't wait out its timeout.
        if (isCurrent(sentence)) send("""{"type":"done","seq":$seq}""")
    }

    private fun clearPlayback() {
        synchronized(playLock) {
            playGeneration++
            playQueue.clear()
            playing?.let { (mp, done) ->
                try { mp.pause() } catch (e: IllegalStateException) { /* already stopped */ }
                done.countDown()
            }
        }
    }

    companion object {
        /** Set by the visible MainActivity; run on Disconnect before the service stops. */
        @Volatile var onDisconnect: (() -> Unit)? = null
        /** Set by MainActivity: PA is talking with the user (`true` = the wake word was just heard). */
        @Volatile var onVoiceActive: ((Boolean) -> Unit)? = null
        /** Set by MainActivity (main thread): the occupant changed — its PA's key, or null when free. */
        var onOccupant: ((String?) -> Unit)? = null
        /** Set by MainActivity (main thread): a PA went on- or offline, or was renamed. */
        var onChanged: (() -> Unit)? = null

        // Main thread only.
        private var instance: AudioService? = null
        private var foreground = false

        /** MainActivity is resumed (a PA's enter_request needs the user looking at the phone). */
        fun setForeground(value: Boolean) {
            if (foreground == value) return
            foreground = value
            instance?.broadcastState()
        }

        fun isOnline(key: String) = instance?.links?.get(key)?.online == true

        /** The user picked PA [key] on the device page. */
        fun enter(key: String) = instance?.enter(key) == true

        /** The user moved off the occupant on the device page (or removed it). */
        fun leave() = instance?.leave()

        /** The paired list changed (pairing, removal, hand-entered address). */
        fun refresh() = instance?.refresh()

        fun start(context: Context) {
            context.startForegroundService(Intent(context, AudioService::class.java))
        }

        private const val ACTION_DISCONNECT = "com.hermes.pocketagent.phone.DISCONNECT"
        private const val TAG = "PocketAgent"
        private const val CHANNEL = "pa_audio"
        private const val NOTIFICATION_ID = 1
        private const val RATE = 16000
        private const val FRAME_BYTES = RATE * 2 / 50 // 20 ms
        private const val RECONNECT_MS = 2000L
        private const val PLAY_TIMEOUT_S = 120L
        /** The occupant offline this long frees the phone (design: 90 s). */
        const val LEASE_MS = 90_000L
    }
}
