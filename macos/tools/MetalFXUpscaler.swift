import Foundation
import Metal
import MetalFX
import CoreImage
import ImageIO
import UniformTypeIdentifiers

// Spatial only. BGRA8 perceptual sRGB, top-left rows via CI's symmetric texture IO.
func emit(_ value: [String: Any]) throws {
    print(String(data: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), encoding: .utf8)!)
}
func fail(_ message: String) throws -> Never { throw NSError(domain: "MetalFX", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
func run() throws {
    let start = Date()
    let args = Array(CommandLine.arguments.dropFirst())
    let device = MTLCreateSystemDefaultDevice()
    let available = device.map { MTLFXSpatialScalerDescriptor.supportsDevice($0) } ?? false
    let identity: [String: Any] = ["engine": "metalfx", "available": available,
        "presets": ["standard"], "reason": available ? "" : "MetalFX spatial is unsupported on this device",
        "backend_version": "koma-metalfx-spatial-v1", "version": "koma-metalfx-spatial-v1", "system": ProcessInfo.processInfo.operatingSystemVersionString,
        "device": device?.name ?? "none", "model": "Apple MetalFX spatial / OS framework"]
    if args == ["--capabilities"] { try emit(identity); return }
    guard available, let device else { try fail("MetalFX spatial unavailable") }
    guard args.count == 10 else { try fail("Expected --input ABS --output ABS --width W --height H --preset standard") }
    var options: [String: String] = [:]
    for i in stride(from: 0, to: args.count, by: 2) { guard options[args[i]] == nil else { try fail("Duplicate argument") }; options[args[i]] = args[i+1] }
    guard let input = options["--input"], let output = options["--output"], input.hasPrefix("/"), output.hasPrefix("/"),
        URL(fileURLWithPath: input).resolvingSymlinksInPath() != URL(fileURLWithPath: output).resolvingSymlinksInPath(),
        !FileManager.default.fileExists(atPath: output), let w = Int(options["--width"] ?? ""), let h = Int(options["--height"] ?? ""),
        w > 0, h > 0, w <= 16384, h <= 16384, options["--preset"] == "standard" else { try fail("Invalid paths, dimensions or unsupported preset; MetalFX supports standard only") }
    guard let ci = CIImage(contentsOf: URL(fileURLWithPath: input), options: [.applyOrientationProperty: true]) else { try fail("Cannot decode image") }
    let iw = Int(ci.extent.width), ih = Int(ci.extent.height)
    guard w >= iw, h >= ih, iw > 0, ih > 0 else { try fail("Spatial scaler requires upscaling") }
    let desc = MTLFXSpatialScalerDescriptor()
    desc.inputWidth = iw; desc.inputHeight = ih; desc.outputWidth = w; desc.outputHeight = h
    desc.colorTextureFormat = .bgra8Unorm; desc.outputTextureFormat = .bgra8Unorm; desc.colorProcessingMode = .perceptual
    guard let scaler = desc.makeSpatialScaler(device: device), let queue = device.makeCommandQueue(), let command = queue.makeCommandBuffer() else { try fail("Cannot create MetalFX pipeline") }
    func texture(_ width: Int, _ height: Int, _ usage: MTLTextureUsage) throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: width, height: height, mipmapped: false)
        d.usage = usage.union([.shaderRead, .shaderWrite, .renderTarget]); d.storageMode = .private
        guard let t = device.makeTexture(descriptor: d) else { try fail("Cannot allocate GPU texture") }; return t
    }
    let source = try texture(iw, ih, scaler.colorTextureUsage), target = try texture(w, h, scaler.outputTextureUsage)
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CIContext(mtlDevice: device)
    context.render(ci.transformed(by: CGAffineTransform(translationX: -ci.extent.minX, y: -ci.extent.minY)), to: source, commandBuffer: command, bounds: CGRect(x: 0,y: 0,width: iw,height: ih), colorSpace: space)
    scaler.colorTexture = source; scaler.outputTexture = target; scaler.inputContentWidth = iw; scaler.inputContentHeight = ih
    scaler.encode(commandBuffer: command)
    command.commit(); command.waitUntilCompleted()
    guard command.status == .completed, command.error == nil else { try fail("MetalFX GPU command failed: \(String(describing: command.error))") }
    guard let result = CIImage(mtlTexture: target, options: [.colorSpace: space]) else { try fail("Cannot read GPU result") }
    try context.writePNGRepresentation(of: result, to: URL(fileURLWithPath: output), format: .RGBA8, colorSpace: space)
    guard let check = CIImage(contentsOf: URL(fileURLWithPath: output)), Int(check.extent.width) == w, Int(check.extent.height) == h else { try fail("Output validation failed") }
    var reply = identity; reply["width"] = w; reply["height"] = h; reply["elapsed_ms"] = Date().timeIntervalSince(start)*1000
    try emit(reply)
}
do { try run() } catch { FileHandle.standardError.write(Data((error.localizedDescription + "\n").utf8)); exit(1) }
