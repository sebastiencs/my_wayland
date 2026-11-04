#version 450
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_texcoord;
layout(std140, set = 0, binding = 1) uniform Blur {
    vec4 transform_c0;
    vec4 transform_c1;
    vec4 transform_c2;
    vec4 tex_c0;
    vec4 tex_c1;
    vec4 tex_c2;
    vec2 item_size;
    vec2 direction;
    vec2 texel_size;
    float radius;
} draw;
layout(location = 0) out vec2 v_texcoord;
void main() {
    mat3 transform = mat3(draw.transform_c0.xyz, draw.transform_c1.xyz, draw.transform_c2.xyz);
    mat3 tex_transform = mat3(draw.tex_c0.xyz, draw.tex_c1.xyz, draw.tex_c2.xyz);
    vec3 position = transform * vec3(a_position, 1.0);
    vec3 texcoord = tex_transform * vec3(a_texcoord, 1.0);
    gl_Position = vec4(position.xy, 0.0, 1.0);
    v_texcoord = texcoord.xy;
}
