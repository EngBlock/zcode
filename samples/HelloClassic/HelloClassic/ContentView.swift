import SwiftUI

struct ContentView: View {
    private let greeter = Greeter(name: "zcode")

    var body: some View {
        Text(greeter.greeting)
            .padding()
            .onAppear { print("HelloClassic launched") }
    }
}
