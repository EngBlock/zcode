import Foundation

/// A tiny model type used to exercise cross-file navigation.
struct Counter {
    private(set) var count = 0

    mutating func increment(by amount: Int = 1) {
        count += amount
    }

    mutating func reset() {
        count = 0
    }
}
