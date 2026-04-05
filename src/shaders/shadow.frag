precision highp float;
varying vec2 v_local_position;
uniform vec4 u_shadow_color;
uniform vec4 u_inner_rect;
uniform float u_corner_radius;
uniform float u_rounding_power;
uniform float u_shadow_range;
uniform float u_shadow_power;
bool pointInRect(vec2 point, vec4 rect) {
    return point.x >= rect.x
        && point.x < (rect.x + rect.z)
        && point.y >= rect.y
        && point.y < (rect.y + rect.w);
}
float roundedCornerDistance(vec2 delta, float roundingPower) {
    vec2 absoluteDelta = abs(delta);
    return pow(pow(absoluteDelta.x, roundingPower) + pow(absoluteDelta.y, roundingPower), 1.0 / roundingPower);
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
    float radius = min(u_corner_radius, min(u_inner_rect.z, u_inner_rect.w) * 0.5);
    float roundingPower = max(u_rounding_power, 0.0001);
    if (pointInRoundedRect(v_local_position, u_inner_rect, radius, roundingPower)) {
        discard;
    }
    float distanceToWindow = shadowDistance(v_local_position, u_inner_rect, radius, roundingPower);
    if (distanceToWindow >= u_shadow_range) {
        discard;
    }
    float alpha = pow(max(1.0 - (distanceToWindow / u_shadow_range), 0.0), u_shadow_power);
    gl_FragColor = u_shadow_color * alpha;
}
