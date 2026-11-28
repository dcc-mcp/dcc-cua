"""State readback from the disposable Blender benchmark scene."""
import json
import os
from pathlib import Path

import bpy


directory = Path(os.environ["DCC_CUA_BLENDER_BENCH"])
bpy.context.preferences.view.show_splash = False
bpy.context.preferences.view.language = "en_US"


def vector(value):
    return [round(float(component), 5) for component in value]


def armature_state(obj):
    bones = [{"name": bone.name, "parent": bone.parent.name if bone.parent else None,
              "connected": bone.use_connect, "head": vector(bone.head_local),
              "tail": vector(bone.tail_local)} for bone in obj.data.bones]
    curves = []
    animation = obj.animation_data
    if animation and animation.action:
        for layer in animation.action.layers:
            for strip in layer.strips:
                bag = strip.channelbag(animation.action_slot)
                for curve in bag.fcurves if bag else ():
                    curves.append({"path": curve.data_path, "index": curve.array_index,
                                   "points": [vector(point.co) for point in curve.keyframe_points],
                                   "mid": round(curve.evaluate(5), 5)})
    return {"bones": bones, "curves": curves}


def read_scene():
    if not (directory / "bound").is_file():
        return 0.05
    context = bpy.context
    obj = context.view_layer.objects.active
    state = {"mode": context.mode, "frame": context.scene.frame_current,
             "objects": sorted(context.scene.objects.keys())}
    viewport = max((area for area in context.window.screen.areas if area.type == "VIEW_3D"),
                   key=lambda area: area.width * area.height, default=None)
    if viewport:
        region = next(region for region in viewport.regions if region.type == "WINDOW")
        state["viewport_fraction"] = [(region.x + region.width / 2) / context.window.width,
                                      1 - (region.y + region.height / 2) / context.window.height]
    if obj:
        state.update(name=obj.name, type=obj.type)
        if obj.type == "MESH":
            heights = [v.co.z for v in obj.data.vertices]
            state.update(vertices=len(heights), faces=len(obj.data.polygons), scale=vector(obj.scale),
                         height=max(heights) - min(heights) if heights else 0)
        elif obj.type == "ARMATURE":
            state.update(armature_state(obj))
    temporary = directory / "scene.tmp"
    if state != read_scene.previous:
        temporary.write_text(json.dumps(state), encoding="utf-8")
        temporary.replace(directory / "scene.json")
        read_scene.previous = state
    return 0.05


read_scene.previous = None
bpy.app.timers.register(read_scene, first_interval=0.1)
