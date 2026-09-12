import Foundation

// #459: the single shared ambient-wind contract for the ranch scene. Every
// moving piece of foliage samples this same deterministic phase of the one
// scene clock; there is no second timer, no per-leaf clock and no
// per-frame randomness. #460's sky motion is expected to sample this
// contract too instead of introducing its own timing source.
enum RanchWind {
    /// Seconds per full gust cycle. Chosen for a gentle but clearly
    /// perceptible sway at phone scale; the previous static canopy carried
    /// no motion signal at all.
    static let period = 5.2
    /// Peak canopy sway in scene points at scale 1 (before the per-cluster
    /// height gradient). Sampled peak-to-peak travel is ~4.9 points.
    static let canopyAmplitude = 2.8
    /// Peak grass-tuft deflection in scene points. Sampled peak-to-peak
    /// travel is ~3.7 points.
    static let grassAmplitude = 2.1

    /// The shared gust phase in radians at the scene clock's elapsed time.
    static func phase(_ elapsed: Double) -> Double {
        elapsed * 2 * .pi / period
    }

    /// Stable per-anchor offset in radians derived from an item's world
    /// position, so neighbouring trees and leaf clusters never march in
    /// lockstep. A pure function of the position: identical across launches
    /// and devices.
    static func anchor(_ worldX: Double) -> Double {
        (worldX.truncatingRemainder(dividingBy: 390) / 390) * 2 * .pi * 0.7
    }

    /// Two-frequency sway in points at `elapsed` for the item at `anchor`
    /// with peak `amplitude`. Both terms are smooth sines of the shared
    /// phase, so the motion is gentle, bounded and clock-driven rather than
    /// random.
    static func sway(elapsed: Double, anchor: Double, amplitude: Double) -> Double {
        let gust = phase(elapsed)
        return amplitude * (0.74 * sin(gust + anchor) + 0.26 * sin(1.93 * gust + anchor * 1.7))
    }

    /// Coherent grass response in points: the same gust travelling across
    /// the field as a spatial delay, not independent per-tuft clocks. The
    /// long wavelength keeps neighbouring tufts moving together.
    static func grassBend(elapsed: Double, worldX: Double,
                          amplitude: Double = RanchWind.grassAmplitude) -> Double {
        let gust = phase(elapsed)
        return amplitude * (0.8 * sin(gust - worldX * 0.028) + 0.2 * sin(1.9 * gust - worldX * 0.011))
    }
}
