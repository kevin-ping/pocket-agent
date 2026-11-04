package com.hermes.pocketagent.phone

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
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
 * The phone as PA's microphone and speaker, over `/audio` (see api/remote.rs):
 *  - up:   PCM16LE 16 kHz mono, 20 ms frames, from the VOICE_COMMUNICATION
 *          source so the platform echo canceller removes PA's own voice and
 *          barge-in keeps working;
 *  - down: `{"type":"play","seq":N}` + one binary mp3 → played in order,
 *          answered with `playing` / `done`; `{"type":"clear"}` stops at once;
 *          `{"type":"snapshot","id":N}` → one camera frame from [CameraHub];
 *  - up:   [CameraHub.stateJson] whenever the camera opens, closes or switches lens.
 * Reconnects on its own; a foreground service so it survives the screen off.
 */
class AudioService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private val client = OkHttpClient.Builder()
        .pingInterval(10, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()

    @Volatile private var running = false
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
        if (running) return START_STICKY
        running = true
        startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)

        audioManager = getSystemService(AudioManager::class.java)
        audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
        @Suppress("DEPRECATION") // setCommunicationDevice is API 31+; Pixel 2 is API 30
        audioManager.isSpeakerphoneOn = true

        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "PocketAgent:audio").apply { acquire() }
        @Suppress("DEPRECATION")
        wifiLock = getSystemService(WifiManager::class.java)
            .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "PocketAgent:audio").apply { acquire() }

        CameraHub.listener = { state -> send(state) }
        recorder = Thread(::recordLoop, "pa-recorder").also { it.start() }
        player = Thread(::playLoop, "pa-player").also { it.start() }
        connect()
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        CameraHub.listener = null
        handler.removeCallbacksAndMessages(null)
        socket?.close(1000, "service stopped")
        socket = null
        clearPlayback()
        player?.interrupt()
        recorder?.join(500)
        wakeLock?.release()
        wifiLock?.release()
        if (::audioManager.isInitialized) audioManager.mode = AudioManager.MODE_NORMAL
        super.onDestroy()
    }

    private fun notification(): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Pocket Agent 语音", NotificationManager.IMPORTANCE_LOW)
        )
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Pocket Agent")
            .setContentText("正在为 Mac 上的 Pocket Agent 收音和播放")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .build()
    }

    // ── Connection ──────────────────────────────────────────────────────────

    private fun connect() {
        if (!running) return
        val config = Config.load(this) ?: return stopSelf()
        client.newWebSocket(Request.Builder().url(config.audioUrl()).build(), object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                Log.i(TAG, "audio connected")
                socket = webSocket
                webSocket.send(CameraHub.stateJson()) // PA starts every phone with the camera closed
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                val msg = try { JSONObject(text) } catch (e: Exception) { return }
                when (msg.optString("type")) {
                    "play" -> expectSeq = msg.getLong("seq")
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
                val seq = expectSeq ?: return
                expectSeq = null
                synchronized(playLock) { playQueue.put(Sentence(seq, bytes.toByteArray(), playGeneration)) }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lost(webSocket, reason)

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
                lost(webSocket, t.message ?: "failure")
        })
    }

    private fun lost(webSocket: WebSocket, why: String) {
        Log.w(TAG, "audio connection lost: $why")
        if (socket === webSocket) socket = null
        // PA has switched back to the Mac's own speaker; nothing queued here still applies.
        clearPlayback()
        if (running) handler.postDelayed(::connect, RECONNECT_MS)
    }

    private fun send(json: String) {
        socket?.send(json)
    }

    // ── Microphone ──────────────────────────────────────────────────────────

    @SuppressLint("MissingPermission") // MainActivity starts the service only after RECORD_AUDIO is granted
    private fun recordLoop() {
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
            while (running) {
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

    private fun playLoop() {
        while (running) {
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

    private companion object {
        const val TAG = "PocketAgent"
        const val CHANNEL = "pa_audio"
        const val NOTIFICATION_ID = 1
        const val RATE = 16000
        const val FRAME_BYTES = RATE * 2 / 50 // 20 ms
        const val RECONNECT_MS = 2000L
        const val PLAY_TIMEOUT_S = 120L
    }
}
