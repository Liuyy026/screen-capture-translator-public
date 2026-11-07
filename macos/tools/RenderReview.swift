import SwiftUI
import AppKit

@main struct ReviewRender {
 @MainActor static func main() throws {
  NSApplication.shared.setActivationPolicy(.prohibited)
  let args = CommandLine.arguments
  struct Reply: Decodable { var regions: [ReadingRegion] }
  let page = try JSONDecoder().decode(PageResult.self, from: Data(contentsOf: URL(fileURLWithPath: args[2])))
  let regions = try JSONDecoder().decode(Reply.self, from: Data(contentsOf: URL(fileURLWithPath: args[3]))).regions
  let items = ReadingLayout.presentations(page: page, regions: regions)
  let image = NSImage(contentsOfFile: args[1])!
  let content = ZStack(alignment: .topLeading) {
   Image(nsImage: image).resizable().frame(width: CGFloat(page.width), height: CGFloat(page.height))
   ForEach(items) { item in ReadingOverlayView(presentation: item, scale: 1, selected: false, select: {}) }
  }.frame(width: CGFloat(page.width), height: CGFloat(page.height)).environment(\.colorScheme, .light)
  let renderer = ImageRenderer(content: content)
  renderer.scale = 1
  let bitmap = NSBitmapImageRep(cgImage: renderer.cgImage!)
  try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[4] + ".png"))
  let renderedIDs = Set(items.flatMap { $0.region.block_ids })
  let audit: [String:Any] = ["width":page.width,"height":page.height,"block_count":page.blocks.count,
   "rendered_ids":Array(renderedIDs).sorted(),"omitted":page.blocks.filter { !renderedIDs.contains($0.id) }.map { ["id":$0.id,"text":$0.translation,"error":$0.error,"box":$0.box] as [String:Any] },
   "items":items.map { item in
    ["id":item.id,"block_ids":item.region.block_ids,"text":item.text,"font":item.layout.fontSize,
     "glyphs":item.layout.glyphs.map { glyph in
      let size = (glyph.text as NSString).size(withAttributes: [.font:NSFont.systemFont(ofSize:item.layout.fontSize,weight:.medium)])
      return ["text":glyph.text,"bounds":[glyph.center.x-size.width/2,glyph.center.y-size.height/2,glyph.center.x+size.width/2,glyph.center.y+size.height/2]] as [String:Any]
     }] as [String:Any]
   }]
  try JSONSerialization.data(withJSONObject:audit, options:[.prettyPrinted,.sortedKeys]).write(to: URL(fileURLWithPath:args[4]+".json"))
 }
}
