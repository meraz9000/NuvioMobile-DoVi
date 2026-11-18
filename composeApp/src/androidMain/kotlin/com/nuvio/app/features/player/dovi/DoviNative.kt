package com.nuvio.app.features.player.dovi

import android.util.Log

/**
 * JNI entry points into `libnuvio_dovi.so`, a thin Rust wrapper around quietvoid's
 * `dolby_vision` crate (the library behind dovi_tool).
 */
internal object DoviNative {
    private const val TAG = "NuvioDovi"

    /** dovi_tool `-m 2`: convert to Profile 8.1 (mapping replaced by identity for FEL). */
    const val MODE_TO_81 = 2

    /** Indices into the `statsOut` array filled by [processAccessUnit]. */
    const val STAT_RPUS_CONVERTED = 0
    const val STAT_RPUS_FAILED = 1
    const val STAT_EL_NALS_DROPPED = 2
    const val STAT_SOURCE_PROFILE = 3
    const val STAT_COUNT = 4

    val isAvailable: Boolean by lazy {
        runCatching { System.loadLibrary("nuvio_dovi") }
            .onFailure { Log.w(TAG, "libnuvio_dovi unavailable; Dolby Vision P7 conversion disabled", it) }
            .isSuccess
    }

    /**
     * Rewrites one HEVC Annex-B access unit: drops UNSPEC63 (enhancement layer) NAL units when
     * [dropEnhancementLayer] is set and converts every UNSPEC62 RPU with dovi_tool [mode].
     * Returns null when the access unit needed no change or could not be processed.
     */
    @JvmStatic
    external fun processAccessUnit(
        data: ByteArray,
        length: Int,
        mode: Int,
        dropEnhancementLayer: Boolean,
        statsOut: IntArray?,
    ): ByteArray?

    /** Dolby Vision profile of the first RPU in an Annex-B access unit, or -1. */
    @JvmStatic
    external fun probeProfile(data: ByteArray, length: Int): Int
}
