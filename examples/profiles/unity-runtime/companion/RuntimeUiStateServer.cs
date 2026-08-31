using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using UnityEngine;
using UnityEngine.SceneManagement;

namespace DccCua.UnityRuntime
{
    [DisallowMultipleComponent]
    public sealed class RuntimeUiStateServer : MonoBehaviour
    {
        private const string SchemaVersion = "1.0.0";

        [SerializeField]
        [Tooltip("Explicit opt-in. The read-only loopback server is disabled by default.")]
        private bool enableStateServer = false;

        [SerializeField]
        [Range(1024, 65535)]
        private int port = 47910;

        [SerializeField]
        [Range(1, 1024)]
        private int maximumWidgets = 1024;

        [SerializeField]
        [Range(0.05f, 5.0f)]
        private float sampleIntervalSeconds = 0.1f;

        private LoopbackJsonStateServer transport;
        private float nextSampleTime;
        private long tickId;
        private long windowId;
        private string lastSemanticJson;
        private int processId;

        [DllImport("user32.dll")]
        private static extern IntPtr GetActiveWindow();

        private void Awake()
        {
            if (!enableStateServer)
            {
                return;
            }

            processId = Process.GetCurrentProcess().Id;
            transport = new LoopbackJsonStateServer(port);
            try
            {
                CaptureSnapshot();
                transport.Start();
            }
            catch (Exception error)
            {
                StopServer();
                enableStateServer = false;
                enabled = false;
                UnityEngine.Debug.LogError("DCC CUA Unity UI state server failed to start: " + error.Message);
            }
        }

        private void Update()
        {
            if (!enableStateServer || Time.unscaledTime < nextSampleTime)
            {
                return;
            }

            nextSampleTime = Time.unscaledTime + sampleIntervalSeconds;
            CaptureSnapshot();
        }

        private void OnDestroy()
        {
            StopServer();
        }

        private void OnApplicationQuit()
        {
            StopServer();
        }

        private void StopServer()
        {
            if (transport != null)
            {
                transport.Dispose();
                transport = null;
            }
        }

        private void CaptureSnapshot()
        {
            IntPtr activeWindow = GetActiveWindow();
            if (activeWindow != IntPtr.Zero)
            {
                windowId = activeWindow.ToInt64();
            }

            UiState state = new UiState
            {
                schemaVersion = SchemaVersion,
                tickId = 0,
                application = new ApplicationIdentity
                {
                    processId = processId,
                    windowId = windowId,
                    productName = UiStateText.Truncate(Application.productName, 256),
                    version = UiStateText.Truncate(Application.version, 64)
                },
                coordinateSpace = new CoordinateSpace
                {
                    width = Math.Max(1, Screen.width),
                    height = Math.Max(1, Screen.height),
                    origin = "top_left",
                    units = "unity_render_pixels"
                },
                scene = UiStateText.Truncate(SceneManager.GetActiveScene().name, 512)
            };
            state.widgets = RuntimeUiCollector.Collect(
                Math.Max(1, Math.Min(maximumWidgets, 1024)),
                state.coordinateSpace.height);

            string semanticJson = JsonUtility.ToJson(state, false);
            if (String.Equals(lastSemanticJson, semanticJson, StringComparison.Ordinal))
            {
                return;
            }

            lastSemanticJson = semanticJson;
            state.tickId = ++tickId;
            transport.Publish(JsonUtility.ToJson(state, false), "\"tick-" + tickId + "\"");
        }
    }
}
