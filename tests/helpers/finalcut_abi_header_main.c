#include "GyroflowFinalCut.h"

_Static_assert(sizeof(GFTime) == 16, "GFTime must remain two signed 64-bit values");
_Static_assert(sizeof(GFFrameGeometry) == 256, "GFFrameGeometry ABI size changed");
_Static_assert(offsetof(GFMetalRenderRequest, geometry) == 136,
               "GFMetalRenderRequest geometry offset changed");
_Static_assert(sizeof(GFRouteDProjectInput) == 40,
               "GFRouteDProjectInput ABI size changed");

int main(void) {
    GFFinalCutInstance *(*create_instance)(GFError **) = gf_finalcut_instance_create;
    void (*free_instance)(GFFinalCutInstance *) = gf_finalcut_instance_free;
    void (*free_error)(GFError *) = gf_finalcut_error_free;
    GFStatus (*load_project)(GFFinalCutInstance *, const uint8_t *, size_t, GFError **) =
        gf_finalcut_instance_load_project;
    uint8_t (*has_project)(const GFFinalCutInstance *) = gf_finalcut_instance_has_project;
    GFStatus (*encode_payload)(const uint8_t *, size_t, GFOwnedBytes *, GFError **) =
        gf_finalcut_project_payload_encode;
    GFStatus (*load_payload)(GFFinalCutInstance *, const uint8_t *, size_t, GFError **) =
        gf_finalcut_instance_load_project_payload;
    GFStatus (*set_parameters)(GFFinalCutInstance *, const GFRenderParameters *, GFError **) =
        gf_finalcut_instance_set_render_parameters;
    GFStatus (*get_parameters)(const GFFinalCutInstance *, GFRenderParameters *, GFError **) =
        gf_finalcut_instance_get_project_render_parameters;
    GFStatus (*decode_payload)(const uint8_t *, size_t, GFOwnedBytes *, GFError **) =
        gf_finalcut_project_payload_decode;
    GFStatus (*patch_fcpxml)(const uint8_t *, size_t, const uint8_t *, size_t,
                            GFRouteDPatchResult *, GFError **) = gf_finalcut_route_d_patch;
    GFStatus (*batch_patch_fcpxml)(const uint8_t *, size_t,
                                  GFRouteDPatchResult *, GFError **) =
        gf_finalcut_route_d_batch_patch;
    GFStatus (*rooted_batch_patch_fcpxml)(const uint8_t *, size_t,
                                         const uint8_t *, size_t,
                                         GFRouteDPatchResult *, GFError **) =
        gf_finalcut_route_d_batch_patch_with_media_roots;
    GFStatus (*snapshot_batch_patch_fcpxml)(const uint8_t *, size_t,
                                           const GFRouteDProjectInput *, size_t,
                                           GFRouteDPatchResult *, GFError **) =
        gf_finalcut_route_d_batch_patch_with_project_inputs;
    GFStatus (*load_timing)(GFFinalCutInstance *, const uint8_t *, size_t,
                           const GFTimeRange *, const GFTimeRange *, GFError **) =
        gf_finalcut_instance_load_timing_payload;
    GFStatus (*resolve_time)(GFFinalCutInstance *, GFTime, GFTime *, GFError **) =
        gf_finalcut_instance_resolve_source_time;
    GFStatus (*render_metal)(GFFinalCutInstance *, const GFMetalRenderRequest *, GFError **) =
        gf_finalcut_instance_render_metal;
    GFRenderParameters parameters = {0};
    GFMetalRenderRequest request = {0};

    (void)create_instance;
    (void)free_instance;
    (void)free_error;
    (void)load_project;
    (void)has_project;
    (void)encode_payload;
    (void)load_payload;
    (void)set_parameters;
    (void)get_parameters;
    (void)decode_payload;
    (void)patch_fcpxml;
    (void)batch_patch_fcpxml;
    (void)rooted_batch_patch_fcpxml;
    (void)snapshot_batch_patch_fcpxml;
    (void)load_timing;
    (void)resolve_time;
    (void)render_metal;
    (void)parameters;
    (void)request;
    return 0;
}
