import Foundation
import AppKit
import CoreImage
import CryptoKit
import Darwin

// Each invocation owns exactly one direct engine child. TERM/INT are forwarded;
// the helper waits for its child before exiting. SIGKILL requires caller tree cleanup.
var cleanupDirectory: URL?
atexit { if let directory = cleanupDirectory { try? FileManager.default.removeItem(at: directory) } }
var childPID: pid_t = 0
var cancelled: Int32 = 0
func onSignal(_ number: Int32) { cancelled = number; if childPID > 0 { kill(childPID, SIGTERM) } }
signal(SIGTERM, onSignal); signal(SIGINT, onSignal)
func fail(_ message: String) -> Never { fputs("RealCUGAN: \(message)\n", stderr); exit(1) }
func json(_ value: [String: Any]) { let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]); print(String(decoding: data, as: UTF8.self)) }
func hash(_ url: URL) -> String { guard let d = try? Data(contentsOf: url) else { return "missing" }; return SHA256.hash(data: d).map { String(format:"%02x", $0) }.joined() }
let start = Date()
let fm = FileManager.default
let executable = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
let root = executable.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
let runtimeAssets = root.appendingPathComponent(".runtime/super-resolution/realcugan")
let releaseAssets = runtimeAssets.appendingPathComponent("realcugan-ncnn-vulkan-20220728-macos")
let fastPNGAssets = runtimeAssets.appendingPathComponent("fast-png")
let fastPNGAvailable = fm.isExecutableFile(atPath: fastPNGAssets.appendingPathComponent("realcugan-ncnn-vulkan").path)
let assets = ProcessInfo.processInfo.environment["KOMA_REALCUGAN_ROOT"].map { URL(fileURLWithPath:$0) }
    ?? (fastPNGAvailable ? fastPNGAssets : releaseAssets)
let engine = assets.appendingPathComponent("realcugan-ncnn-vulkan")
let models = assets.appendingPathComponent("models-se")
let version = "koma-realcugan-1"
let presets = ["conservative", "standard", "denoise"]
let required = ["up2x-no-denoise", "up2x-conservative", "up2x-denoise3x", "up3x-no-denoise", "up3x-conservative", "up3x-denoise3x", "up4x-no-denoise", "up4x-conservative", "up4x-denoise3x"]
let available = fm.isExecutableFile(atPath:engine.path) && required.allSatisfy { name in ["bin", "param"].allSatisfy { fm.fileExists(atPath:models.appendingPathComponent("\(name).\($0)").path) } }
let modelHashes = required.flatMap { name in ["bin", "param"].map { hash(models.appendingPathComponent("\(name).\($0)")) } }.joined()
let identity = SHA256.hash(data:Data(modelHashes.utf8)).map { String(format:"%02x", $0) }.joined()
if CommandLine.arguments.dropFirst() == ["--capabilities"] { json(["engine":"realcugan", "available":available, "presets":available ? presets : [], "reason":available ? "" : "Run macos/scripts/prepare_realcugan.sh", "backend_version":version, "engine_sha256":hash(engine), "model_manifest_sha256":identity,"model_sha256":identity, "models":"models-se/20220728", "scales":[2,3,4]]); exit(0) }
let args = Array(CommandLine.arguments.dropFirst())
var opts = [String:String]()
guard args.count == 10 else { fail("Expected --input ABS --output ABS --width W --height H --preset conservative|standard|denoise") }
for i in stride(from:0, to:args.count, by:2) { guard ["--input","--output","--width","--height","--preset"].contains(args[i]), opts[args[i]] == nil else { fail("Unknown or duplicate option") }; opts[args[i]] = args[i+1] }
guard let input = opts["--input"], let output = opts["--output"], input.hasPrefix("/"), output.hasPrefix("/"), let width = Int(opts["--width"] ?? ""), let height = Int(opts["--height"] ?? ""), width > 0, height > 0, width <= 16384, height <= 16384, let preset = opts["--preset"], presets.contains(preset) else { fail("Invalid paths, size, or preset") }
let inputURL = URL(fileURLWithPath:input).resolvingSymlinksInPath(), outputURL = URL(fileURLWithPath:output).resolvingSymlinksInPath()
guard inputURL != outputURL, !fm.fileExists(atPath:outputURL.path) else { fail("Output must be a new file distinct from input") }
guard available else { fail("Engine/models unavailable") }
let context = CIContext(options:[.useSoftwareRenderer:false])
guard let source = CIImage(contentsOf:inputURL, options:[.applyOrientationProperty:true]) else { fail("Cannot decode input") }
let sw = Int(source.extent.width), sh = Int(source.extent.height)
let ratio = max(Double(width)/Double(sw), Double(height)/Double(sh))
guard ratio > 1, ratio <= 4.00001 else { fail("Requires enlargement of at most 4x; caller must skip images already at target") }
let scale = [2,3,4].first { Double($0) >= ratio - 0.00001 }!
let noise = preset == "conservative" ? -1 : (preset == "standard" ? 0 : 3)
let tmp = outputURL.deletingLastPathComponent().appendingPathComponent(".koma-realcugan-\(UUID().uuidString)")
do { try fm.createDirectory(at:tmp, withIntermediateDirectories:true) } catch { fail(error.localizedDescription) }
cleanupDirectory = tmp
let normalized = tmp.appendingPathComponent("input.png"), enlarged = tmp.appendingPathComponent("inferred.png")
let color = CGColorSpace(name:CGColorSpace.sRGB)!
do { try context.writePNGRepresentation(of:source, to:normalized, format:.RGBA8, colorSpace:color) } catch { fail(error.localizedDescription) }
let engineArguments = [engine.path,"-i",normalized.path,"-o",enlarged.path,"-m",models.path,"-s",String(scale),"-n",String(noise),"-t","128","-j","1:1:1","-g","0"]
// Foundation Process creates a separate process group on macOS. Use posix_spawn
// without SETPGROUP so the engine inherits Rust's per-job group even for SIGKILL.
var actions: posix_spawn_file_actions_t?
posix_spawn_file_actions_init(&actions)
posix_spawn_file_actions_adddup2(&actions, STDERR_FILENO, STDOUT_FILENO)
var attributes: posix_spawnattr_t?
posix_spawnattr_init(&attributes)
var empty = sigset_t(); sigemptyset(&empty)
var defaults = sigset_t(); sigemptyset(&defaults); sigaddset(&defaults,SIGTERM); sigaddset(&defaults,SIGINT)
posix_spawnattr_setsigmask(&attributes, &empty)
posix_spawnattr_setsigdefault(&attributes, &defaults)
posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF))
var argv = engineArguments.map { strdup($0) } + [nil]
var envp = ProcessInfo.processInfo.environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
var blocked = sigset_t(); sigemptyset(&blocked); sigaddset(&blocked,SIGTERM); sigaddset(&blocked,SIGINT)
var previous = sigset_t(); sigprocmask(SIG_BLOCK,&blocked,&previous)
if cancelled != 0 { exit(128 + cancelled) }
var spawned: pid_t = 0
let spawnError = posix_spawn(&spawned, engine.path, &actions, &attributes, &argv, &envp)
if spawnError == 0 { childPID = spawned }
sigprocmask(SIG_SETMASK,&previous,nil)
// A signal may have been delivered on a Core Image worker during spawn.
if cancelled != 0 && spawned > 0 { kill(spawned, SIGTERM) }
for pointer in argv { free(pointer) }
for pointer in envp { free(pointer) }
posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attributes)
guard spawnError == 0 else { fail("Cannot launch engine: \(String(cString:strerror(spawnError)))") }
var status: Int32 = 0
var waited: pid_t
repeat { waited = waitpid(spawned, &status, 0) } while waited == -1 && errno == EINTR
childPID = 0
if cancelled != 0 { exit(128 + cancelled) }
guard waited == spawned, status == 0, let inferred = CIImage(contentsOf:enlarged) else { fail("Engine failed (wait status): \(status)") }
let filter = CIFilter(name:"CILanczosScaleTransform")!
filter.setValue(inferred, forKey:kCIInputImageKey)
filter.setValue(Double(height)/inferred.extent.height, forKey:kCIInputScaleKey)
filter.setValue((Double(width)/inferred.extent.width)/(Double(height)/inferred.extent.height), forKey:kCIInputAspectRatioKey)
guard let result = filter.outputImage?.cropped(to:CGRect(x:0,y:0,width:width,height:height)) else { fail("Resize failed") }
if cancelled != 0 { exit(128 + cancelled) }
let finalPNG = tmp.appendingPathComponent("final.png")
do {
    try context.writePNGRepresentation(of:result, to:finalPNG, format:.RGBA8, colorSpace:color)
    // Same-filesystem copy with O_EXCL semantics; never replace an existing destination.
    try fm.copyItem(at:finalPNG, to:outputURL)
} catch { fail(error.localizedDescription) }
if cancelled != 0 { try? fm.removeItem(at:outputURL); exit(128 + cancelled) }
json(["engine":"realcugan","width":width,"height":height,"elapsed_ms":Date().timeIntervalSince(start)*1000,"preset":preset,"scale":scale,"backend_version":version,"model_manifest_sha256":identity,"model_sha256":identity,"engine_sha256":hash(engine)])
