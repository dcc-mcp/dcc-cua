using System;
using System.Collections.Generic;

namespace DccCua.UnityRuntime
{
    [Serializable]
    internal sealed class UiState
    {
        public string schemaVersion;
        public long tickId;
        public ApplicationIdentity application;
        public CoordinateSpace coordinateSpace;
        public string scene;
        public List<WidgetState> widgets;
    }

    [Serializable]
    internal sealed class ApplicationIdentity
    {
        public int processId;
        public long windowId;
        public string productName;
        public string version;
    }

    [Serializable]
    internal sealed class CoordinateSpace
    {
        public int width;
        public int height;
        public string origin;
        public string units;
    }

    [Serializable]
    internal sealed class WidgetState
    {
        public string id;
        public string path;
        public string kind;
        public string label;
        public string labelSource;
        public bool interactable;
        public WidgetRect rect;
    }

    [Serializable]
    internal sealed class WidgetRect
    {
        public float x;
        public float y;
        public float width;
        public float height;
    }

    internal struct UiLabelValue
    {
        internal readonly string value;
        internal readonly string source;

        internal UiLabelValue(string value, string source)
        {
            this.value = value;
            this.source = source;
        }
    }

    internal static class UiStateText
    {
        internal static string Truncate(string value, int maximum)
        {
            string safe = value ?? String.Empty;
            if (safe.Length <= maximum)
            {
                return safe;
            }
            int length = maximum;
            if (length > 0 && Char.IsHighSurrogate(safe[length - 1]))
            {
                --length;
            }
            return safe.Substring(0, length);
        }
    }
}
