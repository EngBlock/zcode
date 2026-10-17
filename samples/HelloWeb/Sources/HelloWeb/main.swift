/// Where the web app's build (`npm run build` in web/) ends up.
struct WebBundle {
    let root: String

    func page(_ name: String) -> String {
        "\(root)/\(name).html"
    }
}

let bundle = WebBundle(root: "web/dist")
print(bundle.page("index"))
