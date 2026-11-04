#version 450
// Clip tests follow src/shaders/texture_clip.frag so the mask lands on the same pixels.
layout(location = 0) in vec2 v_texcoord;
layout(location = 1) in vec2 v_local_position;
layout(location = 0) out vec4 out_color;
layout(set = 0, binding = 0) uniform sampler2D u_texture;
layout(std140, set = 0, binding = 1) uniform Draw {
    vec4 transform_c0;
    vec4 transform_c1;
    vec4 transform_c2;
    vec4 tex_c0;
    vec4 tex_c1;
    vec4 tex_c2;
    vec2 item_size;
    float alpha;
    int clip_mode;
    vec4 outer_rect;
    vec4 inner_rect;
    float outer_radius;
    float outer_power;
    float inner_radius;
    float inner_power;
} draw;
bool pointInRect(vec2 point, vec4 rect) {
    return point.x >= rect.x
        && point.x < (rect.x + rect.z)
        && point.y >= rect.y
        && point.y < (rect.y + rect.w);
}
float roundedCornerDistance(vec2 delta, float roundingPower) {
    vec2 absoluteDelta = abs(delta);
    return pow(
        pow(absoluteDelta.x, roundingPower) + pow(absoluteDelta.y, roundingPower),
        1.0 / roundingPower
    );
}
bool pointInRoundedRect(vec2 point, vec4 rect, float radius, float roundingPower) {
    if (!pointInRect(point, rect)) {
        return false;
    }
    if (radius <= 0.0) {
        return true;
    }
    float left = rect.x + radius;
    float top = rect.y + radius;
    float right = rect.x + rect.z - radius;
    float bottom = rect.y + rect.w - radius;
    if (point.x < left && point.y < top) {
        return roundedCornerDistance(point - vec2(left, top), roundingPower) <= radius;
    }
    if (point.x >= right && point.y < top) {
        return roundedCornerDistance(point - vec2(right, top), roundingPower) <= radius;
    }
    if (point.x < left && point.y >= bottom) {
        return roundedCornerDistance(point - vec2(left, bottom), roundingPower) <= radius;
    }
    if (point.x >= right && point.y >= bottom) {
        return roundedCornerDistance(point - vec2(right, bottom), roundingPower) <= radius;
    }
    return true;
}
void main() {
    vec2 point = v_local_position;
    if (draw.clip_mode == 3) {
        // Corner-only rounding: keep pixels outside the window box so that
        // client-drawn CSD shadows survive; discard only pixels inside the
        // box that fall outside a rounded corner.
        float outerRadius = min(draw.outer_radius, min(draw.outer_rect.z, draw.outer_rect.w) * 0.5);
        float outerRoundingPower = max(draw.outer_power, 0.0001);
        if (pointInRect(point, draw.outer_rect)
            && !pointInRoundedRect(point, draw.outer_rect, outerRadius, outerRoundingPower)) {
            discard;
        }
    } else if (draw.clip_mode != 0) {
        float outerRadius = min(draw.outer_radius, min(draw.outer_rect.z, draw.outer_rect.w) * 0.5);
        float outerRoundingPower = max(draw.outer_power, 0.0001);
        if (!pointInRoundedRect(point, draw.outer_rect, outerRadius, outerRoundingPower)) {
            discard;
        }
        if (draw.clip_mode == 2) {
            float innerRadius =
                min(draw.inner_radius, min(draw.inner_rect.z, draw.inner_rect.w) * 0.5);
            float innerRoundingPower = max(draw.inner_power, 0.0001);
            if (pointInRoundedRect(point, draw.inner_rect, innerRadius, innerRoundingPower)) {
                discard;
            }
        }
    }
    vec4 color = texture(u_texture, v_texcoord);
    color *= draw.alpha;
    out_color = color;
}
