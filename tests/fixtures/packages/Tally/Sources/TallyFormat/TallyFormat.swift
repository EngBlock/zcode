import Tally

/// Formats a tally as tally marks.
public func marks(_ tally: Tally) -> String {
    String(repeating: "|", count: tally.count)
}
