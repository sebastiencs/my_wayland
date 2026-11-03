#version 450
// Column layout matches Mat3::to_gl_array, the same packing GLES uploads.
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_texcoord;
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
layout(location = 0) out vec2 v_texcoord;
layout(location = 1) out vec2 v_local_position;
void main() {
    mat3 transform = mat3(draw.transform_c0.xyz, draw.transform_c1.xyz, draw.transform_c2.xyz);
    mat3 tex_transform = mat3(draw.tex_c0.xyz, draw.tex_c1.xyz, draw.tex_c2.xyz);
    vec3 position = transform * vec3(a_position, 1.0);
    vec3 texcoord = tex_transform * vec3(a_texcoord, 1.0);
    gl_Position = vec4(position.xy, 0.0, 1.0);
    v_texcoord = texcoord.xy;
    v_local_position = a_position * draw.item_size;
}
