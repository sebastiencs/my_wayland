#version 450
layout(location = 0) in vec2 v_local_position;
layout(location = 0) out vec4 out_color;
layout(std140, set = 0, binding = 0) uniform Shadow {
    vec4 transform_c0;
    vec4 transform_c1;
    vec4 transform_c2;
    vec2 item_size;
    float corner_radius;
    float rounding_power;
    float shadow_range;
    float shadow_power;
    vec4 shadow_color;
    vec4 inner_rect;
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
float shadowDistance(vec2 point, vec4 rect, float radius, float roundingPower) {
    float left = rect.x + radius;
    float top = rect.y + radius;
    float right = rect.x + rect.z - radius;
    float bottom = rect.y + rect.w - radius;
    if (point.x < left && point.y < top) {
        return max(roundedCornerDistance(point - vec2(left, top), roundingPower) - radius, 0.0);
    }
    if (point.x >= right && point.y < top) {
        return max(roundedCornerDistance(point - vec2(right, top), roundingPower) - radius, 0.0);
    }
    if (point.x < left && point.y >= bottom) {
        return max(roundedCornerDistance(point - vec2(left, bottom), roundingPower) - radius, 0.0);
    }
    if (point.x >= right && point.y >= bottom) {
        return max(roundedCornerDistance(point - vec2(right, bottom), roundingPower) - radius, 0.0);
    }
    if (point.x < rect.x) {
        return rect.x - point.x;
    }
    if (point.x >= rect.x + rect.z) {
        return point.x - (rect.x + rect.z);
    }
    if (point.y < rect.y) {
        return rect.y - point.y;
    }
    if (point.y >= rect.y + rect.w) {
        return point.y - (rect.y + rect.w);
    }
    return 0.0;
}
void main() {
    float radius = min(draw.corner_radius, min(draw.inner_rect.z, draw.inner_rect.w) * 0.5);
    float roundingPower = max(draw.rounding_power, 0.0001);
    if (pointInRoundedRect(v_local_position, draw.inner_rect, radius, roundingPower)) {
        discard;
    }
    float distanceToWindow =
        shadowDistance(v_local_position, draw.inner_rect, radius, roundingPower);
    if (distanceToWindow >= draw.shadow_range) {
        discard;
    }
    float alpha = pow(max(1.0 - (distanceToWindow / draw.shadow_range), 0.0), draw.shadow_power);
    out_color = draw.shadow_color * alpha;
}
