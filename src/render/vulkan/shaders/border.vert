#version 450
layout(location = 0) in vec2 a_position;
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
layout(location = 0) out vec2 v_local_position;
void main() {
    mat3 transform = mat3(draw.transform_c0.xyz, draw.transform_c1.xyz, draw.transform_c2.xyz);
    vec3 position = transform * vec3(a_position, 1.0);
    gl_Position = vec4(position.xy, 0.0, 1.0);
    v_local_position = a_position * draw.item_size;
}
