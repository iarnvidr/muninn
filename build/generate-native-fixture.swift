import AVFoundation
import CoreVideo
import Foundation

@main
struct GenerateNativeFixture {
    static func fail(_ message: String) -> Never {
        fputs("native fixture: \(message)\n", stderr)
        exit(1)
    }

    static func main() async {
        guard CommandLine.arguments.count == 2 else {
            fail("usage: generate-native-fixture.swift /path/to/output.mp4")
        }
        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        try? FileManager.default.removeItem(at: output)
        do {
            let writer = try AVAssetWriter(outputURL: output, fileType: .mp4)
            let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
                AVVideoCodecKey: AVVideoCodecType.h264,
                AVVideoWidthKey: 64,
                AVVideoHeightKey: 64,
                AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 100_000]
            ])
            let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32ARGB,
                kCVPixelBufferWidthKey as String: 64,
                kCVPixelBufferHeightKey as String: 64,
                kCVPixelBufferCGImageCompatibilityKey as String: true,
                kCVPixelBufferCGBitmapContextCompatibilityKey as String: true
            ])
            guard writer.canAdd(input) else { fail("cannot add video input") }
            writer.add(input)
            guard writer.startWriting() else { fail(writer.error?.localizedDescription ?? "startWriting failed") }
            writer.startSession(atSourceTime: .zero)
            for frame in 0..<900 {
                while !input.isReadyForMoreMediaData { Thread.sleep(forTimeInterval: 0.002) }
                var optionalBuffer: CVPixelBuffer?
                guard CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, adaptor.pixelBufferPool!, &optionalBuffer) == kCVReturnSuccess,
                      let buffer = optionalBuffer else { fail("pixel buffer allocation failed") }
                CVPixelBufferLockBaseAddress(buffer, [])
                let bytes = CVPixelBufferGetBaseAddress(buffer)!.assumingMemoryBound(to: UInt8.self)
                let stride = CVPixelBufferGetBytesPerRow(buffer)
                for y in 0..<64 { for x in 0..<64 {
                    let p = bytes + y * stride + x * 4
                    p[0] = 255
                    p[1] = UInt8((x * 4 + frame) & 255)
                    p[2] = UInt8((y * 4 + frame) & 255)
                    p[3] = UInt8((x + y + frame) & 255)
                } }
                CVPixelBufferUnlockBaseAddress(buffer, [])
                guard adaptor.append(buffer, withPresentationTime: CMTime(value: Int64(frame), timescale: 30)) else {
                    fail(writer.error?.localizedDescription ?? "frame append failed")
                }
            }
            input.markAsFinished()
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                writer.finishWriting { continuation.resume() }
            }
            guard writer.status == .completed else { fail(writer.error?.localizedDescription ?? "finishWriting failed") }
            let asset = AVURLAsset(url: output)
            let duration = try await asset.load(.duration)
            guard let track = try await asset.loadTracks(withMediaType: .video).first else { fail("output has no video track") }
            let size = try await track.load(.naturalSize)
            let playable = try await asset.load(.isPlayable)
            guard playable, abs(CMTimeGetSeconds(duration) - 30) < 0.05,
                  abs(size.width) == 64, abs(size.height) == 64 else {
                fail("verification failed: playable=\(playable), duration=\(CMTimeGetSeconds(duration)), size=\(size)")
            }
            print("Generated and verified \(output.path): H.264, duration=\(CMTimeGetSeconds(duration))s, dimensions=64x64, playable=true, audio=none")
        } catch { fail(error.localizedDescription) }
    }
}

