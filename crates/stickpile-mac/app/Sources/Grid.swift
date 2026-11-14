import AppKit
import ImageIO

/// Thumbnails by key (item id or pack cover key), decoded off the main
/// thread to the size they are drawn at. Files are remembered, images are
/// kept up to a limit and decoded again when needed.
final class ThumbStore {
    var onReady: ((String) -> Void)?
    /// Largest side of a decoded thumbnail, in pixels.
    var pixelSize: CGFloat = 256

    private var paths: [String: String] = [:]
    private var images: [String: NSImage] = [:]
    private var order: [String] = []
    private var loading: Set<String> = []
    private var failed: Set<String> = []
    private let limit = 900
    private let queue: OperationQueue = {
        let q = OperationQueue()
        q.maxConcurrentOperationCount = 4
        q.qualityOfService = .userInitiated
        return q
    }()

    func set(key: String, path: String) {
        paths[key] = path
        failed.remove(path)
        images[key] = nil
        load(key)
    }

    /// The image if it is decoded; starts decoding it otherwise.
    func image(_ key: String) -> NSImage? {
        if let image = images[key] { return image }
        load(key)
        return nil
    }

    private func load(_ key: String) {
        guard let path = paths[key], !loading.contains(key), !failed.contains(path) else { return }
        loading.insert(key)
        let size = pixelSize
        queue.addOperation { [weak self] in
            let image = ThumbStore.decode(path, maxPixels: size)
            DispatchQueue.main.async {
                guard let self else { return }
                self.loading.remove(key)
                guard self.paths[key] == path else { return }
                guard let image else {
                    self.failed.insert(path)
                    return
                }
                self.store(key, image)
                self.onReady?(key)
            }
        }
    }

    private func store(_ key: String, _ image: NSImage) {
        if images.updateValue(image, forKey: key) == nil {
            order.append(key)
        }
        if order.count > limit {
            let drop = order.prefix(order.count - limit)
            for k in drop { images[k] = nil }
            order.removeFirst(drop.count)
        }
    }

    static func decode(_ path: String, maxPixels: CGFloat) -> NSImage? {
        let url = URL(fileURLWithPath: path) as CFURL
        guard let src = CGImageSourceCreateWithURL(url, nil) else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: Int(maxPixels),
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}

/// `columns` equal columns over the whole width, rows of `cellHeight`.
final class FixedGridLayout: NSCollectionViewLayout {
    var columns = 6
    var cellHeight: CGFloat = 144
    let inset: CGFloat = 6
    private var count = 0
    private var width: CGFloat = 0
    private var columnWidth: CGFloat = 0

    override func prepare() {
        super.prepare()
        guard let cv = collectionView else { return }
        count = cv.numberOfSections > 0 ? cv.numberOfItems(inSection: 0) : 0
        width = cv.enclosingScrollView?.contentView.bounds.width ?? cv.bounds.width
        columnWidth = floor(max(1, width - 2 * inset) / CGFloat(max(1, columns)))
    }

    override var collectionViewContentSize: NSSize {
        let rows = (count + max(1, columns) - 1) / max(1, columns)
        return NSSize(width: width, height: CGFloat(rows) * cellHeight + 2 * inset)
    }

    func frame(_ i: Int) -> NSRect {
        let c = max(1, columns)
        return NSRect(x: inset + CGFloat(i % c) * columnWidth, y: inset + CGFloat(i / c) * cellHeight,
                      width: columnWidth, height: cellHeight)
    }

    override func layoutAttributesForElements(in rect: NSRect) -> [NSCollectionViewLayoutAttributes] {
        guard count > 0, cellHeight > 0 else { return [] }
        let c = max(1, columns)
        let first = max(0, Int(floor((rect.minY - inset) / cellHeight))) * c
        let last = min(count - 1, (Int(floor((rect.maxY - inset) / cellHeight)) + 1) * c - 1)
        guard first <= last else { return [] }
        return (first...last).compactMap { layoutAttributesForItem(at: IndexPath(item: $0, section: 0)) }
    }

    override func layoutAttributesForItem(at indexPath: IndexPath) -> NSCollectionViewLayoutAttributes? {
        guard indexPath.item < count else { return nil }
        let a = NSCollectionViewLayoutAttributes(forItemWith: indexPath)
        a.frame = frame(indexPath.item)
        return a
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: NSRect) -> Bool {
        newBounds.width != width
    }
}

/// The collection view: clicks select and double clicks activate; it
/// never takes the keyboard focus (the popup handles the keys).
final class GridCollectionView: NSCollectionView {
    var onSelect: ((Int) -> Void)?
    var onActivate: ((Int) -> Void)?

    override var acceptsFirstResponder: Bool { false }
    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let index = indexPathForItem(at: p)?.item else { return }
        if event.clickCount == 2 {
            onActivate?(index)
        } else if event.clickCount == 1 {
            onSelect?(index)
        }
    }

    override func keyDown(with event: NSEvent) {}
}

final class GridItem: NSCollectionViewItem {
    static let id = NSUserInterfaceItemIdentifier("cell")
    let cell = GridCell()

    override func loadView() {
        view = cell
    }
}

/// One tile, drawn in one go: selection, thumbnail, emojis, heart, tags, badge.
final class GridCell: NSView {
    enum Kind { case items, packs }

    var kind = Kind.items
    var key = ""
    var entry: EntryView?
    var pack: PackView?
    var selected = false { didSet { if oldValue != selected { needsDisplay = true } } }
    var thumbPx: CGFloat = 128
    var tiles = Tiles(favorites: true, emojis: true, tags: true)
    weak var thumbs: ThumbStore?
    weak var grid: GridCollectionView?

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with event: NSEvent) {
        grid?.mouseDown(with: event)
    }

    override func draw(_ dirtyRect: NSRect) {
        let box = bounds.insetBy(dx: 2, dy: 2)
        if selected {
            NSColor.controlAccentColor.withAlphaComponent(0.45).setFill()
            NSBezierPath(roundedRect: box, xRadius: 10, yRadius: 10).fill()
        }
        let side = max(8, min(thumbPx, bounds.width - cellExtra, bounds.height - cellExtra - (kind == .packs ? packLabels : 0)))
        let area = NSRect(x: floor((bounds.width - side) / 2), y: cellExtra / 2, width: side, height: side)
        let image = thumbs?.image(key)
        if let image {
            let fit = aspectFit(image.size, in: area)
            let dimmed = entry?.incompatible == true
            image.draw(in: fit, from: .zero, operation: .sourceOver, fraction: dimmed ? 0.4 : 1,
                       respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
        } else {
            NSColor.labelColor.withAlphaComponent(0.06).setFill()
            NSBezierPath(roundedRect: area.insetBy(dx: side * 0.06, dy: side * 0.06), xRadius: 8, yRadius: 8).fill()
            if kind == .packs, let icon = symbol("photo.on.rectangle.angled", size: max(14, side / 4),
                                                   color: .tertiaryLabelColor) {
                let s = icon.size
                icon.draw(in: NSRect(x: area.midX - s.width / 2, y: area.midY - s.height / 2, width: s.width, height: s.height),
                          from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
        }
        switch kind {
        case .items: drawEntryMarks(area)
        case .packs: drawPackLabels(below: area)
        }
    }

    private func drawEntryMarks(_ area: NSRect) {
        guard let e = entry else { return }
        let inner = area.insetBy(dx: 3, dy: 3)
        let pillBg = NSColor.black.withAlphaComponent(0.55)
        var bottomLeft = inner.minX
        if tiles.emojis, !e.emojis.isEmpty {
            let font = NSFont.systemFont(ofSize: 12)
            let w = min(inner.width * 0.6, textWidth(e.emojis, font) + 8)
            drawPill(e.emojis, in: NSRect(x: inner.minX, y: inner.minY, width: w, height: 18), font: font,
                     fg: .white, bg: pillBg, radius: 6)
        }
        if tiles.favorites, e.liked {
            let r = NSRect(x: inner.maxX - 20, y: inner.minY, width: 20, height: 18)
            pillBg.setFill()
            NSBezierPath(roundedRect: r, xRadius: 6, yRadius: 6).fill()
            if let heart = symbol("heart.fill", size: 10, weight: .bold,
                                  color: NSColor(srgbRed: 1, green: 0.33, blue: 0.44, alpha: 1)) {
                let s = heart.size
                heart.draw(in: NSRect(x: r.midX - s.width / 2, y: r.midY - s.height / 2, width: s.width, height: s.height),
                           from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
        }
        if e.incompatible, let warn = symbol("exclamationmark.triangle.fill", size: 12, weight: .bold,
                                             color: .systemYellow) {
            let s = warn.size
            let r = NSRect(x: inner.minX, y: inner.maxY - s.height, width: s.width, height: s.height)
            warn.draw(in: r, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            bottomLeft = r.maxX + 3
        }
        var badgeWidth: CGFloat = 0
        if let badge = e.badge, !badge.isEmpty {
            let font = NSFont.systemFont(ofSize: 9, weight: .bold)
            badgeWidth = textWidth(badge, font) + 10
            drawPill(badge, in: NSRect(x: inner.maxX - badgeWidth, y: inner.maxY - 15, width: badgeWidth, height: 15),
                     font: font, fg: .white, bg: pillBg, radius: 5)
        }
        if tiles.tags, !e.tags.isEmpty {
            let font = NSFont.systemFont(ofSize: 9, weight: .medium)
            let room = inner.maxX - bottomLeft - (badgeWidth > 0 ? badgeWidth + 3 : 0)
            let w = min(room, textWidth(e.tags, font) + 10)
            if w > 16 {
                drawPill(e.tags, in: NSRect(x: bottomLeft, y: inner.maxY - 15, width: w, height: 15),
                         font: font, fg: .white, bg: pillBg, radius: 5)
            }
        }
    }

    private func drawPackLabels(below area: NSRect) {
        guard let p = pack else { return }
        let x = bounds.minX + 6
        let w = bounds.width - 12
        let name = NSAttributedString(string: p.name, attributes: lineAttributes(
            font: .systemFont(ofSize: 12, weight: .semibold), color: .labelColor, align: .center))
        name.draw(with: NSRect(x: x, y: area.maxY + 4, width: w, height: 16),
                  options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        let detail = NSAttributedString(string: p.detail, attributes: lineAttributes(
            font: .systemFont(ofSize: 11), color: selected ? NSColor.labelColor.withAlphaComponent(0.75) : .secondaryLabelColor,
            align: .center))
        detail.draw(with: NSRect(x: x, y: area.maxY + 21, width: w, height: 15),
                    options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    private func textWidth(_ s: String, _ font: NSFont) -> CGFloat {
        ceil((s as NSString).size(withAttributes: [.font: font]).width)
    }

    private func aspectFit(_ size: NSSize, in r: NSRect) -> NSRect {
        guard size.width > 0, size.height > 0 else { return r }
        let scale = min(r.width / size.width, r.height / size.height)
        let w = size.width * scale, h = size.height * scale
        return NSRect(x: r.midX - w / 2, y: r.midY - h / 2, width: w, height: h)
    }
}

/// A scrolling grid of items or packs.
final class GridView: NSScrollView, NSCollectionViewDataSource {
    let kind: GridCell.Kind
    let collection = GridCollectionView()
    let grid = FixedGridLayout()
    let thumbs: ThumbStore
    private(set) var entries: [EntryView] = []
    private(set) var packs: [PackView] = []
    private(set) var selected: Int?
    private var thumbPx: CGFloat = 128
    private var tiles = Tiles(favorites: true, emojis: true, tags: true)
    /// The usable size changed (width and height without the insets).
    var onSize: ((NSSize) -> Void)?

    var count: Int { kind == .items ? entries.count : packs.count }

    init(kind: GridCell.Kind, thumbs: ThumbStore) {
        self.kind = kind
        self.thumbs = thumbs
        super.init(frame: .zero)
        hasVerticalScroller = true
        hasHorizontalScroller = false
        autohidesScrollers = true
        drawsBackground = false
        borderType = .noBorder
        collection.collectionViewLayout = grid
        collection.dataSource = self
        collection.isSelectable = false
        collection.backgroundColors = [.clear]
        collection.register(GridItem.self, forItemWithIdentifier: GridItem.id)
        documentView = collection
    }

    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    override func tile() {
        super.tile()
        let s = contentView.bounds.size
        onSize?(NSSize(width: max(0, s.width - 2 * grid.inset), height: max(0, s.height - 2 * grid.inset)))
    }

    func setEntries(_ e: [EntryView]) {
        entries = e
        selected = nil
        collection.reloadData()
    }

    func setPacks(_ p: [PackView]) {
        packs = p
        selected = nil
        collection.reloadData()
    }

    func configure(columns: Int, thumbPx: CGFloat, tiles: Tiles) {
        let height = thumbPx + cellExtra + (kind == .packs ? packLabels : 0)
        let changed = grid.columns != columns || grid.cellHeight != height || self.thumbPx != thumbPx
        let tilesChanged = self.tiles != tiles
        self.thumbPx = thumbPx
        self.tiles = tiles
        if changed {
            grid.columns = max(1, columns)
            grid.cellHeight = height
            grid.invalidateLayout()
        }
        if changed || tilesChanged {
            for item in collection.visibleItems() {
                guard let cell = (item as? GridItem)?.cell else { continue }
                cell.thumbPx = thumbPx
                cell.tiles = tiles
                cell.needsDisplay = true
            }
        }
    }

    func setSelected(_ index: Int?) {
        guard index != selected else { return }
        let old = selected
        selected = index
        for i in [old, index].compactMap({ $0 }) where i < count {
            (collection.item(at: IndexPath(item: i, section: 0)) as? GridItem)?.cell.selected = i == index
        }
    }

    /// Scroll so the selected cell is in view; to the top without one.
    func scrollToSelected() {
        layoutSubtreeIfNeeded()
        collection.layoutSubtreeIfNeeded()
        guard let i = selected, i < count else {
            contentView.scroll(to: .zero)
            reflectScrolledClipView(contentView)
            return
        }
        let f = grid.frame(i).insetBy(dx: 0, dy: -grid.inset)
        collection.scrollToVisible(f)
        debug("scroll to \(i): clip \(contentView.bounds), cells \(collection.visibleItems().count)")
    }

    func thumbnailReady(_ key: String) {
        for item in collection.visibleItems() {
            if let cell = (item as? GridItem)?.cell, cell.key == key {
                cell.needsDisplay = true
            }
        }
    }

    // MARK: data source

    func collectionView(_ cv: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        count
    }

    func collectionView(_ cv: NSCollectionView, itemForRepresentedObjectAt indexPath: IndexPath) -> NSCollectionViewItem {
        let item = cv.makeItem(withIdentifier: GridItem.id, for: indexPath) as! GridItem
        let cell = item.cell
        let i = indexPath.item
        cell.kind = kind
        cell.thumbs = thumbs
        cell.grid = collection
        cell.thumbPx = thumbPx
        cell.tiles = tiles
        cell.selected = i == selected
        switch kind {
        case .items:
            let e = entries[i]
            cell.entry = e
            cell.pack = nil
            cell.key = e.id
            cell.toolTip = e.tooltip
        case .packs:
            let p = packs[i]
            cell.pack = p
            cell.entry = nil
            cell.key = p.coverKey
            cell.toolTip = p.detail.isEmpty ? p.name : p.name + "\n" + p.detail
        }
        cell.needsDisplay = true
        return item
    }
}
