#ifndef GYROFLOW_FINAL_CUT_H
#define GYROFLOW_FINAL_CUT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct GFFinalCutInstance GFFinalCutInstance;

typedef enum GFStatus {
    GF_STATUS_OK = 0,
    GF_STATUS_NULL_POINTER = 1,
    GF_STATUS_INVALID_ARGUMENT = 2,
    GF_STATUS_INVALID_PROJECT = 3,
    GF_STATUS_UNKNOWN_PAYLOAD_VERSION = 4,
    GF_STATUS_MISSING_TIMING = 5,
    GF_STATUS_STALE_TIMING = 6,
    GF_STATUS_UNSUPPORTED_PIXEL_FORMAT = 7,
    GF_STATUS_RENDER_FAILED = 8,
    GF_STATUS_ROUTE_D_INVALID_INPUT = 9,
    GF_STATUS_ROUTE_D_UNSAFE_STRUCTURE = 10,
    GF_STATUS_ROUTE_D_NO_UPDATEABLE_TARGETS = 11,
    GF_STATUS_PANIC = 255
} GFStatus;

typedef struct GFError {
    GFStatus code;
    char *message;
} GFError;

typedef struct GFOwnedBytes {
    uint8_t *data;
    size_t len;
    size_t capacity;
} GFOwnedBytes;

typedef struct GFRouteDPatchResult {
    GFOwnedBytes fcpxml;
    GFOwnedBytes report;
} GFRouteDPatchResult;

typedef uint32_t GFRouteDProjectInputStatus;
#define GF_ROUTE_D_PROJECT_INPUT_AVAILABLE ((GFRouteDProjectInputStatus)0)
#define GF_ROUTE_D_PROJECT_INPUT_MISSING ((GFRouteDProjectInputStatus)1)
#define GF_ROUTE_D_PROJECT_INPUT_PERMISSION_DENIED ((GFRouteDProjectInputStatus)2)
#define GF_ROUTE_D_PROJECT_INPUT_TOO_LARGE ((GFRouteDProjectInputStatus)3)

typedef struct GFRouteDProjectInput {
    const uint8_t *path_bytes;
    size_t path_len;
    const uint8_t *project_bytes;
    size_t project_len;
    GFRouteDProjectInputStatus status;
    uint32_t reserved;
} GFRouteDProjectInput;

typedef struct GFTime {
    int64_t numerator;
    int64_t denominator;
} GFTime;

typedef struct GFTimeRange {
    GFTime start;
    GFTime duration;
} GFTimeRange;

typedef struct GFRenderParameters {
    double fov;
    double smoothness;
    double lens_correction;
    double horizon_lock_amount;
    double horizon_lock_roll;
    int32_t zoom_mode;
    uint8_t overview;
    uint8_t reserved[3];
} GFRenderParameters;

#define GF_FRAME_GEOMETRY_VERSION_LEGACY ((uint32_t)0)
#define GF_FRAME_GEOMETRY_VERSION ((uint32_t)1)

typedef uint32_t GFGeometryValidity;
#define GF_GEOMETRY_VALIDITY_LEGACY_UNKNOWN ((GFGeometryValidity)0)
#define GF_GEOMETRY_VALIDITY_VALID ((GFGeometryValidity)1)
#define GF_GEOMETRY_VALIDITY_INVALID ((GFGeometryValidity)2)

typedef uint32_t GFGeometrySupport;
#define GF_GEOMETRY_SUPPORT_UNKNOWN ((GFGeometrySupport)0)
#define GF_GEOMETRY_SUPPORT_AFFINE_2D ((GFGeometrySupport)1)
#define GF_GEOMETRY_SUPPORT_PERSPECTIVE_UNVERIFIED ((GFGeometrySupport)2)
#define GF_GEOMETRY_SUPPORT_UNSUPPORTED ((GFGeometrySupport)3)

typedef uint32_t GFImageOrigin;
#define GF_IMAGE_ORIGIN_BOTTOM_LEFT ((GFImageOrigin)0)
#define GF_IMAGE_ORIGIN_TOP_LEFT ((GFImageOrigin)2)
#define GF_IMAGE_ORIGIN_UNKNOWN ((GFImageOrigin)UINT32_MAX)

typedef int32_t GFRotationDegrees;
#define GF_ROTATION_NONE ((GFRotationDegrees)0)
#define GF_ROTATION_CLOCKWISE_90 ((GFRotationDegrees)90)
#define GF_ROTATION_180 ((GFRotationDegrees)180)
#define GF_ROTATION_CLOCKWISE_270 ((GFRotationDegrees)270)
#define GF_ROTATION_UNKNOWN ((GFRotationDegrees)INT32_MIN)

typedef struct GFDimensionsU32 {
    uint32_t width;
    uint32_t height;
} GFDimensionsU32;

typedef struct GFProjectGeometry {
    GFDimensionsU32 input_dimensions;
    GFDimensionsU32 output_dimensions;
    GFRotationDegrees video_rotation;
    uint32_t reserved;
} GFProjectGeometry;

typedef struct GFRectI32 {
    int32_t left;
    int32_t bottom;
    int32_t right;
    int32_t top;
} GFRectI32;

/* Row-major homogeneous 2D affine transform. */
typedef struct GFAffineTransform {
    double values[9];
} GFAffineTransform;

/*
 * A per-frame geometry snapshot. Version 0 is valid only when every field is
 * zero-initialized and means legacy/unknown geometry; consumers must preserve the
 * pre-geometry render behavior.
 *
 * Version 1 carries a normalized forward affine from the current frame after
 * input_rotation, in oriented source ideal square-pixel coordinates, to the
 * current callback destination ideal square-pixel coordinates. inverse_transform
 * maps in the opposite direction. Both are row-major 3x3 matrices operating on
 * absolute pixel-boundary coordinates with +x right and +y up. source_rect and
 * destination_rect are absolute tile bounds in those domains; their image-origin
 * fields define how each Metal tile is rebased to tile-local coordinates.
 * tile_dimensions is the shared physical Metal texture extent.
 *
 * The affine describes only the alignment that this effect callback must compose
 * after Gyroflow stabilization. It is not a snapshot of the complete Final Cut
 * project/editorial state and must not be baked into Gyroflow FOV, adaptive zoom,
 * project payload, or Route D timing. The FxPlug adapter is responsible for
 * producing this normalized contract from the current callback's host matrices.
 */
typedef struct GFFrameGeometry {
    uint32_t version;
    uint32_t struct_size;
    GFGeometryValidity validity;
    GFGeometrySupport support;
    GFDimensionsU32 source_dimensions;
    GFDimensionsU32 oriented_dimensions;
    GFDimensionsU32 tile_dimensions;
    GFDimensionsU32 output_dimensions;
    GFRectI32 source_rect;
    GFRectI32 destination_rect;
    GFImageOrigin source_origin;
    GFImageOrigin destination_origin;
    GFRotationDegrees input_rotation;
    GFRotationDegrees video_rotation;
    GFAffineTransform forward_transform;
    GFAffineTransform inverse_transform;
    uint32_t reserved[4];
} GFFrameGeometry;

typedef struct GFMetalRenderRequest {
    void *input_texture;
    void *output_texture;
    void *command_queue;
    uint64_t device_registry_id;
    uint32_t width;
    uint32_t height;
    uint32_t input_row_bytes;
    uint32_t output_row_bytes;
    uint32_t pixel_format;
    GFTime effect_local_time;
    GFTimeRange effect_bounds;
    GFTimeRange input_bounds;
    GFFrameGeometry geometry;
} GFMetalRenderRequest;

GFFinalCutInstance *gf_finalcut_instance_create(GFError **out_error);
void gf_finalcut_instance_free(GFFinalCutInstance *instance);
GFStatus gf_finalcut_instance_load_project(
    GFFinalCutInstance *instance,
    const uint8_t *project_bytes,
    size_t project_len,
    GFError **out_error
);
uint8_t gf_finalcut_instance_has_project(const GFFinalCutInstance *instance);
GFStatus gf_finalcut_instance_get_project_geometry(
    const GFFinalCutInstance *instance,
    GFProjectGeometry *out_geometry,
    GFError **out_error
);
GFStatus gf_finalcut_project_payload_encode(
    const uint8_t *project_bytes,
    size_t project_len,
    GFOwnedBytes *out_payload,
    GFError **out_error
);
GFStatus gf_finalcut_project_payload_decode(
    const uint8_t *payload_bytes,
    size_t payload_len,
    GFOwnedBytes *out_project,
    GFError **out_error
);
GFStatus gf_finalcut_instance_load_project_payload(
    GFFinalCutInstance *instance,
    const uint8_t *payload_bytes,
    size_t payload_len,
    GFError **out_error
);
GFStatus gf_finalcut_instance_set_render_parameters(
    GFFinalCutInstance *instance,
    const GFRenderParameters *parameters,
    GFError **out_error
);
GFStatus gf_finalcut_instance_get_project_render_parameters(
    const GFFinalCutInstance *instance,
    GFRenderParameters *out_parameters,
    GFError **out_error
);
GFStatus gf_finalcut_route_d_patch(
    const uint8_t *input_bytes,
    size_t input_len,
    const uint8_t *processed_name_bytes,
    size_t processed_name_len,
    GFRouteDPatchResult *out_result,
    GFError **out_error
);
GFStatus gf_finalcut_route_d_batch_patch(
    const uint8_t *input_bytes,
    size_t input_len,
    GFRouteDPatchResult *out_result,
    GFError **out_error
);
GFStatus gf_finalcut_route_d_batch_patch_with_media_roots(
    const uint8_t *input_bytes,
    size_t input_len,
    const uint8_t *media_roots_json_bytes,
    size_t media_roots_json_len,
    GFRouteDPatchResult *out_result,
    GFError **out_error
);
GFStatus gf_finalcut_route_d_batch_patch_with_project_inputs(
    const uint8_t *input_bytes,
    size_t input_len,
    const GFRouteDProjectInput *project_inputs,
    size_t project_inputs_len,
    GFRouteDPatchResult *out_result,
    GFError **out_error
);
void gf_finalcut_route_d_patch_result_free(GFRouteDPatchResult *result);
GFStatus gf_finalcut_instance_load_timing_payload(
    GFFinalCutInstance *instance,
    const uint8_t *payload_bytes,
    size_t payload_len,
    const GFTimeRange *observed_effect_bounds,
    const GFTimeRange *observed_input_bounds,
    GFError **out_error
);
GFStatus gf_finalcut_instance_resolve_source_time(
    GFFinalCutInstance *instance,
    GFTime effect_local_time,
    GFTime *out_source_time,
    GFError **out_error
);
GFStatus gf_finalcut_instance_render_metal(
    GFFinalCutInstance *instance,
    const GFMetalRenderRequest *request,
    GFError **out_error
);
void gf_finalcut_error_free(GFError *error);
void gf_finalcut_owned_bytes_free(GFOwnedBytes *bytes);

#ifdef __cplusplus
}
#endif

#endif
