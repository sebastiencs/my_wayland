#version 450
layout(location = 0) in vec2 v_local_position;
layout(location = 0) out vec4 out_color;
layout(std140, set = 0, binding = 0) uniform Border {
    vec4 transform_c0;
    vec4 transform_c1;
    vec4 transform_c2;
    vec2 item_size;
    float outer_radius;
    float outer_power;
    float inner_radius;
    float inner_power;
    float angle;
    float alpha;
    vec4 outer_rect;
    vec4 inner_rect;
    vec4 gradient_start;
    vec4 gradient_end;
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
    float outerRadius = min(draw.outer_radius, min(draw.outer_rect.z, draw.outer_rect.w) * 0.5);
    float outerRoundingPower = max(draw.outer_power, 0.0001);
    if (!pointInRoundedRect(v_local_position, draw.outer_rect, outerRadius, outerRoundingPower)) {
        discard;
    }
    float innerRadius = min(draw.inner_radius, min(draw.inner_rect.z, draw.inner_rect.w) * 0.5);
    float innerRoundingPower = max(draw.inner_power, 0.0001);
    if (pointInRoundedRect(v_local_position, draw.inner_rect, innerRadius, innerRoundingPower)) {
        discard;
    }
    vec2 normalized = (v_local_position / max(draw.item_size, vec2(1.0, 1.0))) - vec2(0.5, 0.5);
    float angle = radians(draw.angle);
    vec2 direction = vec2(cos(angle), sin(angle));
    float progress = clamp(dot(normalized, direction) + 0.5, 0.0, 1.0);
    out_color = mix(draw.gradient_start, draw.gradient_end, progress) * draw.alpha;
}
