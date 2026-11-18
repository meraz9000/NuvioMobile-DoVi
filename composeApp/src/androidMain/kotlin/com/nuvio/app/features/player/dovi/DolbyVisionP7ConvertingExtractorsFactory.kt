@file:OptIn(androidx.media3.common.util.UnstableApi::class)

package com.nuvio.app.features.player.dovi

import android.net.Uri
import android.util.Log
import androidx.media3.common.C
import androidx.media3.common.DataReader
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.ParsableByteArray
import androidx.media3.extractor.Extractor
import androidx.media3.extractor.ExtractorInput
import androidx.media3.extractor.ExtractorOutput
import androidx.media3.extractor.ExtractorsFactory
import androidx.media3.extractor.PositionHolder
import androidx.media3.extractor.SeekMap
import androidx.media3.extractor.SniffFailure
import androidx.media3.extractor.TrackOutput
import com.nuvio.app.features.player.RewrappableExtractor
import java.io.EOFException
import kotlin.math.max

/**
 * On-the-fly Dolby Vision Profile 7 -> Profile 8.1 conversion, the same approach Kodi uses on
 * Android:
 *
 * 1. The container extractor (Matroska, MP4, ...) hands each HEVC access unit to us in Annex-B form.
 * 2. Enhancement-layer NAL units (UNSPEC63) are dropped; Profile 8.1 has no EL.
 * 3. Every RPU (UNSPEC62) is rewritten to Profile 8.1 by dovi_tool's conversion (`-m 2`).
 * 4. The track [Format] is relabelled from `dvhe.07.xx` to `dvhe.08.xx`, so MediaCodec picks the
 *    device's Dolby Vision Profile 8 decoder (or the HEVC HDR10 base layer on devices without one).
 *
 * Nothing changes for tracks that are not Dolby Vision Profile 7, or when the native library is
 * missing.
 */
internal class DolbyVisionP7ConvertingExtractorsFactory(
    private val delegate: ExtractorsFactory,
) : ExtractorsFactory {
    override fun createExtractors(): Array<Extractor> = wrap(delegate.createExtractors())

    override fun createExtractors(uri: Uri, responseHeaders: Map<String, List<String>>): Array<Extractor> =
        wrap(delegate.createExtractors(uri, responseHeaders))

    private fun wrap(extractors: Array<Extractor>): Array<Extractor> =
        Array(extractors.size) { index -> DoviConvertingExtractor(extractors[index]) }

    companion object {
        /** Wraps [factory] when conversion is wanted and the native library loads. */
        fun wrapIfEnabled(factory: ExtractorsFactory, enabled: Boolean): ExtractorsFactory {
            val available = enabled && DoviNative.isAvailable
            Log.i("NuvioDovi", "DV7->8.1 conversion enabled=$enabled nativeLoaded=$available")
            return if (available) DolbyVisionP7ConvertingExtractorsFactory(factory) else factory
        }
    }
}

private class DoviConvertingExtractor(
    private val delegate: Extractor,
) : RewrappableExtractor {
    override val wrappedExtractor: Extractor get() = delegate
    override fun rewrap(inner: Extractor): Extractor = DoviConvertingExtractor(inner)
    override fun sniff(input: ExtractorInput) = delegate.sniff(input)
    override fun getSniffFailureDetails(): List<SniffFailure> = delegate.getSniffFailureDetails()
    override fun init(output: ExtractorOutput) = delegate.init(DoviConvertingExtractorOutput(output))
    override fun read(input: ExtractorInput, seekPosition: PositionHolder) = delegate.read(input, seekPosition)
    override fun seek(position: Long, timeUs: Long) = delegate.seek(position, timeUs)
    override fun release() = delegate.release()
    override fun getUnderlyingImplementation(): Extractor = delegate.getUnderlyingImplementation()
}

private class DoviConvertingExtractorOutput(
    private val delegate: ExtractorOutput,
) : ExtractorOutput {
    private val videoTracks = mutableMapOf<Int, TrackOutput>()

    override fun track(id: Int, type: Int): TrackOutput {
        val output = delegate.track(id, type)
        return if (type == C.TRACK_TYPE_VIDEO) {
            videoTracks.getOrPut(id) { DoviConvertingTrackOutput(output, id) }
        } else {
            output
        }
    }

    override fun endTracks() = delegate.endTracks()
    override fun seekMap(seekMap: SeekMap) = delegate.seekMap(seekMap)
}

internal object DolbyVisionFormats {
    private val PROFILE_7_CODECS = Regex("""^(dvhe|dvh1)\.07\.(\d+)""", RegexOption.IGNORE_CASE)

    fun isProfile7(format: Format): Boolean {
        val codecs = format.codecs ?: return false
        if (!PROFILE_7_CODECS.containsMatchIn(codecs)) return false
        return format.sampleMimeType == MimeTypes.VIDEO_DOLBY_VISION ||
            format.sampleMimeType == MimeTypes.VIDEO_H265
    }

    /** `dvhe.07.06` -> `dvhe.08.06`; the DV level is kept. */
    fun toProfile81Codecs(codecs: String): String =
        PROFILE_7_CODECS.replace(codecs) { match -> "${match.groupValues[1]}.08.${match.groupValues[2]}" }

    fun toProfile81(format: Format): Format =
        format.buildUpon()
            .setSampleMimeType(MimeTypes.VIDEO_DOLBY_VISION)
            .setCodecs(toProfile81Codecs(format.codecs!!))
            .build()
}

/**
 * Buffers each sample of a Dolby Vision Profile 7 video track, rewrites it with [DoviNative] and
 * forwards the result. Samples that can't be handled safely (encrypted, split across
 * [sampleMetadata] offsets, or carrying supplemental data) pass through unchanged.
 */
private class DoviConvertingTrackOutput(
    private val delegate: TrackOutput,
    private val trackId: Int,
) : TrackOutput {
    private var active = false
    private var pending = ByteArray(0)
    private var pendingLength = 0
    /** Set when a non-main sample part arrived; the current sample is then forwarded unchanged. */
    private var currentSampleRaw = false

    private val stats = IntArray(DoviNative.STAT_COUNT)
    private val scratch = ParsableByteArray()
    private var samples = 0L
    private var rpusConverted = 0L
    private var rpusFailed = 0L
    private var elNalsDropped = 0L
    private var nativeFailures = 0L

    override fun format(format: Format) {
        Log.i(
            TAG,
            "Track $trackId video format: mime=${format.sampleMimeType} codecs=${format.codecs} " +
                "${format.width}x${format.height} color=${format.colorInfo}",
        )
        val convert = DolbyVisionFormats.isProfile7(format)
        if (!convert && active) flushPendingRaw()
        active = convert
        if (convert) {
            val converted = DolbyVisionFormats.toProfile81(format)
            Log.i(TAG, "Track $trackId: Dolby Vision ${format.codecs} -> ${converted.codecs} (dovi_tool mode ${DoviNative.MODE_TO_81})")
            delegate.format(converted)
        } else {
            delegate.format(format)
        }
    }

    override fun durationUs(durationUs: Long) = delegate.durationUs(durationUs)

    override fun sampleData(input: DataReader, length: Int, allowEndOfInput: Boolean, sampleDataPart: Int): Int {
        if (!active) return delegate.sampleData(input, length, allowEndOfInput, sampleDataPart)
        if (sampleDataPart != TrackOutput.SAMPLE_DATA_PART_MAIN) {
            markCurrentSampleRaw()
            return delegate.sampleData(input, length, allowEndOfInput, sampleDataPart)
        }
        if (currentSampleRaw) return delegate.sampleData(input, length, allowEndOfInput, sampleDataPart)
        ensureCapacity(pendingLength + length)
        val read = input.read(pending, pendingLength, length)
        if (read == C.RESULT_END_OF_INPUT) {
            if (allowEndOfInput) return C.RESULT_END_OF_INPUT
            throw EOFException()
        }
        pendingLength += read
        return read
    }

    override fun sampleData(data: ParsableByteArray, length: Int, sampleDataPart: Int) {
        if (!active) return delegate.sampleData(data, length, sampleDataPart)
        if (sampleDataPart != TrackOutput.SAMPLE_DATA_PART_MAIN) {
            markCurrentSampleRaw()
            return delegate.sampleData(data, length, sampleDataPart)
        }
        if (currentSampleRaw) return delegate.sampleData(data, length, sampleDataPart)
        ensureCapacity(pendingLength + length)
        data.readBytes(pending, pendingLength, length)
        pendingLength += length
    }

    override fun sampleMetadata(
        timeUs: Long,
        flags: Int,
        size: Int,
        offset: Int,
        cryptoData: TrackOutput.CryptoData?,
    ) {
        if (!active) return delegate.sampleMetadata(timeUs, flags, size, offset, cryptoData)

        val passThrough = currentSampleRaw ||
            cryptoData != null ||
            offset != 0 ||
            size != pendingLength ||
            (flags and C.BUFFER_FLAG_HAS_SUPPLEMENTAL_DATA) != 0
        if (passThrough) {
            flushPendingRaw()
            currentSampleRaw = false
            delegate.sampleMetadata(timeUs, flags, size, offset, cryptoData)
            return
        }

        val rewritten = try {
            DoviNative.processAccessUnit(pending, pendingLength, DoviNative.MODE_TO_81, true, stats)
        } catch (t: Throwable) {
            nativeFailures++
            if (nativeFailures == 1L) Log.e(TAG, "Native Dolby Vision conversion failed", t)
            null
        }
        samples++
        rpusConverted += stats[DoviNative.STAT_RPUS_CONVERTED]
        rpusFailed += stats[DoviNative.STAT_RPUS_FAILED]
        elNalsDropped += stats[DoviNative.STAT_EL_NALS_DROPPED]
        stats.fill(0)

        val outBytes = rewritten ?: pending
        val outLength = rewritten?.size ?: pendingLength
        writeToDelegate(outBytes, outLength)
        pendingLength = 0
        delegate.sampleMetadata(timeUs, flags, outLength, 0, null)

        if (samples == 1L || samples % LOG_EVERY_SAMPLES == 0L) {
            Log.i(
                TAG,
                "Track $trackId: $samples samples, $rpusConverted RPUs -> 8.1, " +
                    "$elNalsDropped EL NALs dropped, $rpusFailed RPU failures",
            )
        }
    }

    private fun markCurrentSampleRaw() {
        if (!currentSampleRaw) {
            flushPendingRaw()
            currentSampleRaw = true
        }
    }

    private fun flushPendingRaw() {
        if (pendingLength > 0) writeToDelegate(pending, pendingLength)
        pendingLength = 0
    }

    private fun writeToDelegate(bytes: ByteArray, length: Int) {
        scratch.reset(bytes, length)
        delegate.sampleData(scratch, length, TrackOutput.SAMPLE_DATA_PART_MAIN)
    }

    private fun ensureCapacity(required: Int) {
        if (pending.size < required) pending = pending.copyOf(max(required, max(pending.size * 2, 64 * 1024)))
    }

    private companion object {
        const val TAG = "NuvioDovi"
        const val LOG_EVERY_SAMPLES = 2_000L
    }
}
