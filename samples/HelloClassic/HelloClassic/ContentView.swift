import SwiftUI

struct ContentView: View {
    private let greeter = Greeter(name: "zcode")

    var body: some View {
        Text(greeter.greeting)
            .padding()
            .onAppear {
                print("HelloClassic launched")
                // What the scheme's Run action passes (Edit Scheme ▸ Arguments).
                let arguments = CommandLine.arguments.dropFirst().joined(separator: " ")
                print("arguments: \(arguments) GREETING=\(ProcessInfo.processInfo.environment["GREETING"] ?? "")")
            }
    }
}
