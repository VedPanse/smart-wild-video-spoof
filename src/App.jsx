import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import "./App.css";

const STREAM_TARGET = "wildsafe-ml-service.onrender.com";
const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];

async function waitForIceGatheringComplete(peerConnection) {
  if (peerConnection.iceGatheringState === "complete") {
    return;
  }

  await new Promise((resolve) => {
    const handleStateChange = () => {
      if (peerConnection.iceGatheringState === "complete") {
        peerConnection.removeEventListener("icegatheringstatechange", handleStateChange);
        resolve();
      }
    };

    peerConnection.addEventListener("icegatheringstatechange", handleStateChange);
  });
}

function preferH264(transceiver) {
  const capabilities = RTCRtpSender.getCapabilities?.("video");
  const codecs = capabilities?.codecs ?? [];
  const h264Codecs = codecs.filter((codec) => codec.mimeType.toLowerCase() === "video/h264");

  if (h264Codecs.length > 0 && transceiver.setCodecPreferences) {
    transceiver.setCodecPreferences(h264Codecs);
  }
}

function forceH264InSdp(sdp) {
  const normalizedSdp = sdp.replace(/\r?\n/g, "\r\n");
  const sections = normalizedSdp.split("\r\nm=");

  return sections
    .map((section, index) => {
      const prefixedSection = index === 0 ? section : `m=${section}`;

      if (!prefixedSection.startsWith("m=video ")) {
        return prefixedSection;
      }

      const lines = prefixedSection.split("\r\n");
      const h264PayloadTypes = new Set();
      const keepPayloadTypes = new Set();

      for (const line of lines) {
        const match = line.match(/^a=rtpmap:(\d+)\s+H264\/90000/i);
        if (match) {
          h264PayloadTypes.add(match[1]);
          keepPayloadTypes.add(match[1]);
        }
      }

      for (const line of lines) {
        const match = line.match(/^a=fmtp:(\d+)\s+apt=(\d+)/i);
        if (match && h264PayloadTypes.has(match[2])) {
          keepPayloadTypes.add(match[1]);
        }
      }

      if (h264PayloadTypes.size === 0) {
        throw new Error("This WebView did not offer an H.264 WebRTC video codec.");
      }

      const filteredLines = lines.filter((line, lineIndex) => {
        if (lineIndex === 0) {
          return true;
        }

        const codecAttribute = line.match(/^a=(?:rtpmap|fmtp|rtcp-fb):(\d+)/);
        return !codecAttribute || keepPayloadTypes.has(codecAttribute[1]);
      });

      const mediaLineParts = filteredLines[0].split(" ");
      filteredLines[0] = [...mediaLineParts.slice(0, 3), ...keepPayloadTypes].join(" ");

      return filteredLines.join("\r\n");
    })
    .join("\r\n");
}

function App() {
  const videoRef = useRef(null);
  const streamRef = useRef(null);
  const peerConnectionRef = useRef(null);
  const [cameraStatus, setCameraStatus] = useState("Requesting camera access...");
  const [backendStatus, setBackendStatus] = useState("Waiting for camera...");
  const [error, setError] = useState("");
  const [requestCount, setRequestCount] = useState(0);

  useEffect(() => {
    let isMounted = true;

    async function start() {
      setError("");
      setCameraStatus("Requesting camera access...");
      setBackendStatus(`Preparing Rust backend WebRTC stream to ${STREAM_TARGET}...`);

      streamRef.current?.getTracks().forEach((track) => track.stop());
      peerConnectionRef.current?.close();
      streamRef.current = null;
      peerConnectionRef.current = null;

      if (!navigator.mediaDevices?.getUserMedia) {
        setCameraStatus("Camera API is not available in this webview.");
        return;
      }

      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          video: {
            width: { ideal: 1280 },
            height: { ideal: 720 },
            frameRate: { ideal: 30 },
          },
          audio: false,
        });

        if (!isMounted) {
          stream.getTracks().forEach((track) => track.stop());
          return;
        }

        streamRef.current = stream;

        if (videoRef.current) {
          videoRef.current.srcObject = stream;
          await videoRef.current.play();
        }

        const [videoTrack] = stream.getVideoTracks();
        setCameraStatus(videoTrack?.label ? `Using camera: ${videoTrack.label}` : "Camera connected.");

        const peerConnection = new RTCPeerConnection({
          iceServers: ICE_SERVERS,
        });
        peerConnectionRef.current = peerConnection;

        peerConnection.addEventListener("connectionstatechange", () => {
          setBackendStatus(`WebRTC connection: ${peerConnection.connectionState}`);
        });

        peerConnection.addEventListener("iceconnectionstatechange", () => {
          if (peerConnection.iceConnectionState === "failed") {
            setError("WebRTC ICE connection failed. If this persists, the deployed service needs TURN relay support.");
          }
        });

        const transceiver = peerConnection.addTransceiver(videoTrack, {
          direction: "sendonly",
          streams: [stream],
        });
        preferH264(transceiver);

        setBackendStatus("Creating H.264 WebRTC offer...");
        const offer = await peerConnection.createOffer();
        const h264Offer = {
          type: offer.type,
          sdp: forceH264InSdp(offer.sdp),
        };

        await peerConnection.setLocalDescription(h264Offer);
        await waitForIceGatheringComplete(peerConnection);

        if (!peerConnection.localDescription?.sdp) {
          throw new Error("Could not create a local WebRTC offer.");
        }

        setBackendStatus(`Sending H.264 offer through Rust backend to ${STREAM_TARGET}...`);
        const answerSdp = await invoke("exchange_h264_webrtc_offer", {
          offerSdp: peerConnection.localDescription.sdp,
        });

        if (!isMounted) {
          return;
        }

        await peerConnection.setRemoteDescription({
          type: "answer",
          sdp: answerSdp,
        });

        setBackendStatus(`Streaming H.264 WebRTC video to ${STREAM_TARGET}.`);
      } catch (cameraOrWebrtcError) {
        console.error("Camera/WebRTC error:", cameraOrWebrtcError);
        setBackendStatus("WebRTC stream failed.");
        setError(cameraOrWebrtcError?.message || String(cameraOrWebrtcError));
      }
    }

    start();

    return () => {
      isMounted = false;
      peerConnectionRef.current?.close();
      streamRef.current?.getTracks().forEach((track) => track.stop());
    };
  }, [requestCount]);

  return (
    <main className="camera-page">
      <header className="camera-header">
        <div>
          <h1>Spoofing video for Smart Wild</h1>
          <p>{cameraStatus}</p>
          <p>{backendStatus}</p>
        </div>
        <button type="button" onClick={() => setRequestCount((count) => count + 1)}>
          Restart stream
        </button>
      </header>

      <section className="camera-frame" aria-label="Webcam preview">
        <video ref={videoRef} autoPlay playsInline muted />
      </section>

      {error ? <p className="camera-error">{error}</p> : null}
    </main>
  );
}

export default App;
