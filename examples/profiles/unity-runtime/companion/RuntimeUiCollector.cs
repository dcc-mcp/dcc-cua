using System;
using System.Collections.Generic;
using System.Globalization;
using UnityEngine;
using UnityEngine.UI;

#if UNITY_2021_1_OR_NEWER
using UiToolkit = UnityEngine.UIElements;
#endif

#if DCC_CUA_TMP
using TMPro;
#endif

namespace DccCua.UnityRuntime
{
    internal static class RuntimeUiCollector
    {
        internal static List<WidgetState> Collect(int maximumWidgets, int viewportHeight)
        {
            List<WidgetState> widgets = new List<WidgetState>();
            Selectable[] selectables = Resources.FindObjectsOfTypeAll<Selectable>();
            foreach (Selectable selectable in selectables)
            {
                if (widgets.Count >= maximumWidgets || selectable == null ||
                    !selectable.gameObject.scene.IsValid() || !selectable.gameObject.activeInHierarchy)
                {
                    continue;
                }

                RectTransform rectTransform = selectable.transform as RectTransform;
                WidgetRect rect;
                if (rectTransform == null || !TryGetUguiRect(rectTransform, viewportHeight, out rect))
                {
                    continue;
                }

                UiLabelValue label = UguiLabel(selectable);
                string path = HierarchyPath(selectable.transform);
                widgets.Add(new WidgetState
                {
                    id = "ugui:" + selectable.GetInstanceID().ToString(CultureInfo.InvariantCulture),
                    path = UiStateText.Truncate(path, 384),
                    kind = UiStateText.Truncate(selectable.GetType().Name, 64),
                    label = UiStateText.Truncate(label.value, 256),
                    labelSource = label.source,
                    interactable = selectable.IsInteractable(),
                    rect = rect
                });
            }

#if UNITY_2021_1_OR_NEWER
            UiToolkit.UIDocument[] documents = Resources.FindObjectsOfTypeAll<UiToolkit.UIDocument>();
            foreach (UiToolkit.UIDocument document in documents)
            {
                if (widgets.Count >= maximumWidgets || document == null ||
                    !document.gameObject.scene.IsValid() || !document.gameObject.activeInHierarchy ||
                    document.rootVisualElement == null)
                {
                    continue;
                }
                CollectToolkitWidgets(document, document.rootVisualElement, widgets, maximumWidgets);
            }
#endif

            widgets.Sort(delegate(WidgetState left, WidgetState right)
            {
                return StringComparer.Ordinal.Compare(left.id, right.id);
            });
            return widgets;
        }

        private static bool TryGetUguiRect(RectTransform transform, int viewportHeight, out WidgetRect rect)
        {
            Vector3[] corners = new Vector3[4];
            transform.GetWorldCorners(corners);
            Canvas canvas = transform.GetComponentInParent<Canvas>();
            Camera camera = canvas != null && canvas.renderMode != RenderMode.ScreenSpaceOverlay
                ? canvas.worldCamera
                : null;
            float minX = Single.PositiveInfinity;
            float minY = Single.PositiveInfinity;
            float maxX = Single.NegativeInfinity;
            float maxY = Single.NegativeInfinity;
            for (int index = 0; index < corners.Length; ++index)
            {
                Vector2 point = RectTransformUtility.WorldToScreenPoint(camera, corners[index]);
                minX = Math.Min(minX, point.x);
                minY = Math.Min(minY, point.y);
                maxX = Math.Max(maxX, point.x);
                maxY = Math.Max(maxY, point.y);
            }
            rect = new WidgetRect
            {
                x = minX,
                y = viewportHeight - maxY,
                width = Math.Max(0.0f, maxX - minX),
                height = Math.Max(0.0f, maxY - minY)
            };
            return IsFinite(rect.x) && IsFinite(rect.y) && IsFinite(rect.width) && IsFinite(rect.height);
        }

        private static UiLabelValue UguiLabel(Selectable selectable)
        {
            InputField input = selectable as InputField;
            if (input != null)
            {
                Text placeholder = input.placeholder as Text;
                if (placeholder != null && !String.IsNullOrWhiteSpace(placeholder.text))
                {
                    return new UiLabelValue(placeholder.text, "placeholder");
                }
                return new UiLabelValue(selectable.gameObject.name, "game_object_name");
            }

            Text[] texts = selectable.GetComponentsInChildren<Text>(true);
            foreach (Text text in texts)
            {
                if (text != null && !String.IsNullOrWhiteSpace(text.text))
                {
                    return new UiLabelValue(text.text, "visible_text");
                }
            }

#if DCC_CUA_TMP
            TMP_InputField tmpInput = selectable.GetComponent<TMP_InputField>();
            if (tmpInput != null)
            {
                TMP_Text placeholder = tmpInput.placeholder as TMP_Text;
                if (placeholder != null && !String.IsNullOrWhiteSpace(placeholder.text))
                {
                    return new UiLabelValue(placeholder.text, "placeholder");
                }
                return new UiLabelValue(selectable.gameObject.name, "game_object_name");
            }
            TMP_Text[] tmpTexts = selectable.GetComponentsInChildren<TMP_Text>(true);
            foreach (TMP_Text text in tmpTexts)
            {
                if (text != null && !String.IsNullOrWhiteSpace(text.text))
                {
                    return new UiLabelValue(text.text, "visible_text");
                }
            }
#endif
            return new UiLabelValue(selectable.gameObject.name, "game_object_name");
        }

#if UNITY_2021_1_OR_NEWER
        private static void CollectToolkitWidgets(
            UiToolkit.UIDocument document,
            UiToolkit.VisualElement element,
            List<WidgetState> widgets,
            int maximumWidgets)
        {
            if (widgets.Count >= maximumWidgets)
            {
                return;
            }
            if (IsToolkitControl(element) && element.enabledInHierarchy &&
                element.resolvedStyle.display != UiToolkit.DisplayStyle.None &&
                element.resolvedStyle.visibility == UiToolkit.Visibility.Visible)
            {
                float scale = document.panelSettings == null ? 1.0f : document.panelSettings.scale;
                Rect bounds = element.worldBound;
                UiLabelValue label = ToolkitLabel(element);
                string path = UiStateText.Truncate(
                    document.gameObject.scene.name + "/" + document.gameObject.name + "/" +
                    ToolkitPath(element),
                    384);
                widgets.Add(new WidgetState
                {
                    id = "uitk:" + element.GetHashCode().ToString(CultureInfo.InvariantCulture),
                    path = path,
                    kind = UiStateText.Truncate(element.GetType().Name, 64),
                    label = UiStateText.Truncate(label.value, 256),
                    labelSource = label.source,
                    interactable = element.enabledInHierarchy,
                    rect = new WidgetRect
                    {
                        x = bounds.x * scale,
                        y = bounds.y * scale,
                        width = Math.Max(0.0f, bounds.width * scale),
                        height = Math.Max(0.0f, bounds.height * scale)
                    }
                });
            }
            foreach (UiToolkit.VisualElement child in element.Children())
            {
                CollectToolkitWidgets(document, child, widgets, maximumWidgets);
                if (widgets.Count >= maximumWidgets)
                {
                    return;
                }
            }
        }

        private static bool IsToolkitControl(UiToolkit.VisualElement element)
        {
            return element is UiToolkit.Button || element is UiToolkit.Toggle ||
                element is UiToolkit.TextField || element is UiToolkit.DropdownField ||
                element is UiToolkit.Slider || element is UiToolkit.Scroller;
        }

        private static UiLabelValue ToolkitLabel(UiToolkit.VisualElement element)
        {
            UiToolkit.TextField textField = element as UiToolkit.TextField;
            if (textField != null)
            {
                return new UiLabelValue(
                    String.IsNullOrWhiteSpace(textField.label) ? element.name : textField.label,
                    String.IsNullOrWhiteSpace(textField.label) ? "element_name" : "visible_text");
            }
            UiToolkit.Button button = element as UiToolkit.Button;
            if (button != null && !String.IsNullOrWhiteSpace(button.text))
            {
                return new UiLabelValue(button.text, "visible_text");
            }
            UiToolkit.Toggle toggle = element as UiToolkit.Toggle;
            if (toggle != null && !String.IsNullOrWhiteSpace(toggle.label))
            {
                return new UiLabelValue(toggle.label, "visible_text");
            }
            UiToolkit.DropdownField dropdown = element as UiToolkit.DropdownField;
            if (dropdown != null && !String.IsNullOrWhiteSpace(dropdown.label))
            {
                return new UiLabelValue(dropdown.label, "visible_text");
            }
            UiToolkit.Slider slider = element as UiToolkit.Slider;
            if (slider != null && !String.IsNullOrWhiteSpace(slider.label))
            {
                return new UiLabelValue(slider.label, "visible_text");
            }
            return new UiLabelValue(element.name ?? String.Empty, "element_name");
        }

        private static string ToolkitPath(UiToolkit.VisualElement element)
        {
            List<string> parts = new List<string>();
            UiToolkit.VisualElement current = element;
            while (current != null)
            {
                int index = current.parent == null ? 0 : current.parent.IndexOf(current);
                string name = String.IsNullOrWhiteSpace(current.name) ? current.GetType().Name : current.name;
                parts.Add(name + "[" + index + "]");
                current = current.parent;
            }
            parts.Reverse();
            return String.Join("/", parts.ToArray());
        }
#endif

        private static string HierarchyPath(Transform transform)
        {
            List<string> parts = new List<string>();
            Transform current = transform;
            while (current != null)
            {
                parts.Add(current.gameObject.name + "[" + current.GetSiblingIndex() + "]");
                current = current.parent;
            }
            parts.Reverse();
            string scene = transform.gameObject.scene.IsValid() ? transform.gameObject.scene.name : "DontDestroyOnLoad";
            return UiStateText.Truncate(scene + "/" + String.Join("/", parts.ToArray()), 384);
        }

        private static bool IsFinite(float value)
        {
            return !Single.IsNaN(value) && !Single.IsInfinity(value);
        }
    }
}
